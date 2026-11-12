import { afterEach, expect, test } from "bun:test";
import { rm } from "node:fs/promises";
import net, { createServer, type Server } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

const originalPlatform = process.platform;
const originalArgv = process.argv;
const originalCreateConnection = net.createConnection;
const originalEnvironment = {
  HERDR_ENV: process.env.HERDR_ENV,
  HERDR_OMP_IDLE_DEBOUNCE_MS: process.env.HERDR_OMP_IDLE_DEBOUNCE_MS,
  HERDR_PANE_ID: process.env.HERDR_PANE_ID,
  HERDR_SOCKET_PATH: process.env.HERDR_SOCKET_PATH,
  OMPCODE: process.env.OMPCODE,
};

let server: Server | undefined;
let socketPath: string | undefined;
let importCounter = 0;

afterEach(async () => {
  await new Promise<void>((resolve, reject) => {
    if (!server) {
      resolve();
      return;
    }
    server.close((error) => (error ? reject(error) : resolve()));
  });
  server = undefined;

  if (socketPath) {
    await rm(socketPath, { force: true });
    socketPath = undefined;
  }

  Object.defineProperty(process, "platform", { value: originalPlatform });
  net.createConnection = originalCreateConnection;
  process.argv = originalArgv;
  for (const [name, value] of Object.entries(originalEnvironment)) {
    if (value === undefined) {
      delete process.env[name];
    } else {
      process.env[name] = value;
    }
  }
});

const integrations = [
  { name: "Pi", modulePath: "./pi/herdr-agent-state.ts" },
  { name: "Oh My Pi", modulePath: "./omp/herdr-agent-state.ts" },
] as const;

const socketPlugins = [
  {
    name: "OpenCode",
    modulePath: "./opencode/herdr-agent-state.js",
    sessionID: "opencode-session",
  },
  { name: "Kilo", modulePath: "./kilo/herdr-agent-state.js", sessionID: "kilo-session" },
] as const;

function importFresh(modulePath: string) {
  importCounter += 1;
  return import(`${modulePath}?test=${importCounter}`);
}

type Handler = (event: unknown, context: unknown) => unknown;

function createExtensionHarness() {
  const handlers = new Map<string, Handler>();
  const eventHandlers = new Map<string, Handler>();
  return {
    handlers,
    eventHandlers,
    pi: {
      on(event: string, handler: Handler) {
        handlers.set(event, handler);
      },
      events: {
        on(event: string, handler: Handler) {
          eventHandlers.set(event, handler);
          return () => {};
        },
      },
    },
  };
}

function configureIntegrationEnvironment(recordingSocketPath: string) {
  // Tests may run inside an OMP shell; nested-session cases opt in explicitly.
  delete process.env.OMPCODE;
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = recordingSocketPath;
  process.env.HERDR_PANE_ID = "test:p1";
}

function captureConnectionEndpoint() {
  let connectedEndpoint: unknown;
  net.createConnection = ((...args: unknown[]) => {
    connectedEndpoint = args[0];
    return Reflect.apply(originalCreateConnection, net, args);
  }) as typeof net.createConnection;
  return () => connectedEndpoint;
}

async function startRecordingServer(name: string, wireSizes?: number[]): Promise<unknown[]> {
  const recordingSocketPath = join(tmpdir(), `herdr-${name}-${process.pid}.sock`);
  socketPath = recordingSocketPath;
  await rm(recordingSocketPath, { force: true });

  const requests: unknown[] = [];
  const recordingServer = createServer((socket) => {
    let input = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      input += chunk;
      const newline = input.indexOf("\n");
      if (newline === -1) {
        return;
      }
      wireSizes?.push(Buffer.byteLength(input.slice(0, newline + 1), "utf8"));
      requests.push(JSON.parse(input.slice(0, newline)));
      socket.end("{}\n");
    });
  });
  server = recordingServer;
  await new Promise<void>((resolve, reject) => {
    recordingServer.once("error", reject);
    recordingServer.listen(originalPlatform === "win32" ? `\\\\.\\pipe\\${recordingSocketPath}` : recordingSocketPath, resolve);
  });
  configureIntegrationEnvironment(recordingSocketPath);
  return requests;
}

for (const socketPlugin of socketPlugins) {
  test(`${socketPlugin.name} maps the Windows socket marker path to a named pipe endpoint`, async () => {
    const markerPath = `herdr-${socketPlugin.name.toLowerCase()}-${process.pid}.sock`;
    configureIntegrationEnvironment(markerPath);
    Object.defineProperty(process, "platform", { value: "win32" });
    const connectedEndpoint = captureConnectionEndpoint();

    process.argv = ["bun", "/$bunfs/root/src/index.js", "run"];
    const { HerdrAgentStatePlugin } = await importFresh(socketPlugin.modulePath);
    const plugin = await HerdrAgentStatePlugin();
    await plugin.event({
      event: {
        type: "session.updated",
        properties: { sessionID: socketPlugin.sessionID },
      },
    });

    expect(connectedEndpoint()).toBe(`\\\\.\\pipe\\${markerPath}`);
  });
}

test("OpenCode stays disabled without the Herdr socket environment", async () => {
  process.env.HERDR_ENV = "1";
  process.env.HERDR_PANE_ID = "test:p1";
  delete process.env.HERDR_SOCKET_PATH;

  const { HerdrAgentStatePlugin } = await importFresh("./opencode/herdr-agent-state.js");

  expect(await HerdrAgentStatePlugin()).toEqual({});
});

for (const integration of integrations) {
  test(`${integration.name} maps the Windows socket marker path to a named pipe endpoint`, async () => {
    const markerPath = `herdr-${integration.name.toLowerCase().replaceAll(" ", "-")}-${process.pid}.sock`;
    configureIntegrationEnvironment(markerPath);
    Object.defineProperty(process, "platform", { value: "win32" });
    const connectedEndpoint = captureConnectionEndpoint();
    const { handlers, pi } = createExtensionHarness();

    const { default: install } = await importFresh(integration.modulePath);
    install(pi);
    await handlers.get("session_start")?.(
      { reason: "startup" },
      {
        hasUI: true,
        mode: "tui",
        isIdle: () => true,
        sessionManager: {
          getSessionFile: () => undefined,
          getSessionId: () => "test-session",
        },
      },
    );

    expect(connectedEndpoint()).toBe(`\\\\.\\pipe\\${markerPath}`);
  });

  test(`${integration.name} reload preserves working state when the agent is active`, async () => {
    const requests = await startRecordingServer(
      integration.name.toLowerCase().replaceAll(" ", "-"),
    );
    const { handlers, pi } = createExtensionHarness();

    const { default: install } = await importFresh(integration.modulePath);
    install(pi);

    const sessionStart = handlers.get("session_start");
    expect(sessionStart).toBeDefined();
    await sessionStart?.(
      { reason: "reload" },
      {
        hasUI: true,
        mode: "tui",
        isIdle: () => false,
        sessionManager: {
          getSessionFile: () => undefined,
          getSessionId: () => undefined,
        },
      },
    );

    const reportedState = () => {
      for (const request of requests) {
        if (!isRecord(request) || request.method !== "pane.report_agent") {
          continue;
        }
        const params = request.params;
        if (isRecord(params) && typeof params.state === "string") {
          return params.state;
        }
      }
      return undefined;
    };

    const deadline = Date.now() + 1_000;
    while (Date.now() < deadline && reportedState() === undefined) {
      await Bun.sleep(5);
    }

    expect(reportedState()).toBe("working");
  });
}

test("OMP ignores nested sessions launched inside another OMP shell", async () => {
  const requests = await startRecordingServer("omp-nested");
  process.env.OMPCODE = "1";
  const { handlers, pi } = createExtensionHarness();

  const { default: install } = await importFresh("./omp/herdr-agent-state.ts");
  install(pi);

  // OMP sets OMPCODE on every shell it spawns. A nested `omp` inherits it and
  // must not claim the pane's session for its short-lived conversation.
  expect(handlers.size).toBe(0);
  await handlers.get("session_start")?.(
    { reason: "startup" },
    {
      hasUI: true,
      isIdle: () => true,
      sessionManager: {
        getSessionFile: () => "/tmp/omp-nested.jsonl",
        getSessionId: () => "omp-nested",
      },
    },
  );
  await Bun.sleep(25);

  expect(requests).toEqual([]);
});

test("OMP accepts POSIX and Windows session paths", async () => {
  const { isAbsoluteSessionPath } = await importFresh("./omp/herdr-agent-state.ts");

  expect(isAbsoluteSessionPath("/tmp/omp-session.jsonl")).toBe(true);
  expect(isAbsoluteSessionPath("C:\\Users\\User\\.omp\\agent\\sessions\\omp-session.jsonl")).toBe(
    true,
  );
  expect(isAbsoluteSessionPath("C:/Users/User/.omp/agent/sessions/omp-session.jsonl")).toBe(true);
  expect(isAbsoluteSessionPath("relative/omp-session.jsonl")).toBe(false);
});

test("Pi reports a Windows session path", async () => {
  const requests = await startRecordingServer("pi-windows-session-path");
  const { handlers, pi } = createExtensionHarness();
  const { default: install } = await importFresh("./pi/herdr-agent-state.ts");
  install(pi);

  const sessionPath = "C:\\Users\\User\\.pi\\agent\\sessions\\pi-session.jsonl";
  await handlers.get("session_start")?.(
    { reason: "startup" },
    {
      ...piContext(() => true),
      sessionManager: {
        getSessionFile: () => sessionPath,
        getSessionId: () => "pi-session",
      },
    },
  );
  await waitFor(() => requests.length === 2);

  expect(requests.map(requestSessionPath)).toEqual([sessionPath, sessionPath]);
});

test("Pi reports idle only after the agent settles", async () => {
  const requests = await startRecordingServer("pi-settled");
  const { handlers, pi } = createExtensionHarness();
  const { default: install } = await importFresh("./pi/herdr-agent-state.ts");
  install(pi);

  expect(completionHandlers(handlers)).toEqual(["agent_settled"]);
  let idle = true;
  const context = piContext(() => idle);
  await handlers.get("session_start")?.({ reason: "startup" }, context);
  await waitFor(() => requestStates(requests).length === 1);

  idle = false;
  handlers.get("agent_start")?.({}, context);
  await waitFor(() => requestStates(requests).length === 2);
  expect(requestStates(requests)).toEqual(["idle", "working"]);
  expect(handlers.has("agent_end")).toBe(false);

  const requestCountBeforeStaleSettlement = requests.length;
  handlers.get("agent_settled")?.({}, context);
  await Bun.sleep(25);
  expect(requests).toHaveLength(requestCountBeforeStaleSettlement);
  expect(requestStates(requests)).toEqual(["idle", "working"]);

  idle = true;
  handlers.get("agent_settled")?.({}, context);
  await waitFor(() => requestStates(requests).length === 3);
  expect(requestStates(requests)).toEqual(["idle", "working", "idle"]);
});

test("Pi ignores RPC sessions even when UI APIs are available", async () => {
  const requests = await startRecordingServer("pi-rpc");
  const { handlers, pi } = createExtensionHarness();
  const { default: install } = await importFresh("./pi/herdr-agent-state.ts");
  install(pi);

  const context = {
    ...piContext(() => true),
    hasUI: true,
    mode: "rpc",
  };
  await handlers.get("session_start")?.({ reason: "startup" }, context);
  handlers.get("agent_start")?.({}, context);
  handlers.get("agent_settled")?.({}, context);
  await Bun.sleep(25);

  expect(requests).toEqual([]);
});

test("Pi settlement preserves explicit blocked-state precedence", async () => {
  const requests = await startRecordingServer("pi-settled-blocked");
  const { eventHandlers, handlers, pi } = createExtensionHarness();
  const { default: install } = await importFresh("./pi/herdr-agent-state.ts");
  install(pi);

  let idle = true;
  const context = piContext(() => idle);
  await handlers.get("session_start")?.({ reason: "startup" }, context);
  await waitFor(() => requestStates(requests).length === 1);
  idle = false;
  handlers.get("agent_start")?.({}, context);
  await waitFor(() => requestStates(requests).length === 2);
  eventHandlers.get("herdr:blocked")?.({ active: true, label: "approval" }, context);
  await waitFor(() => requestStates(requests).length === 3);

  idle = true;
  handlers.get("agent_settled")?.({}, context);
  await Bun.sleep(25);
  expect(requestStates(requests)).toEqual(["idle", "working", "blocked"]);

  eventHandlers.get("herdr:blocked")?.({ active: false }, context);
  await waitFor(() => requestStates(requests).length === 4);
  expect(requestStates(requests)).toEqual(["idle", "working", "blocked", "idle"]);
});

test("Pi reports the session replacement source", async () => {
  const requests = await startRecordingServer("pi-session-source");
  const { handlers, pi } = createExtensionHarness();

  const { default: install } = await importFresh("./pi/herdr-agent-state.ts");
  install(pi);

  const sessionStart = handlers.get("session_start");
  expect(sessionStart).toBeDefined();
  await sessionStart?.(
    { reason: "new" },
    {
      hasUI: true,
      mode: "tui",
      isIdle: () => true,
      sessionManager: {
        getSessionFile: () => "/tmp/pi-new.jsonl",
        getSessionId: () => "pi-new",
      },
    },
  );

  const reportedSession = () =>
    requests.find((request) => isRecord(request) && request.method === "pane.report_agent_session");
  const deadline = Date.now() + 1_000;
  while (Date.now() < deadline && reportedSession() === undefined) {
    await Bun.sleep(5);
  }

  const request = reportedSession();
  expect(request).toBeDefined();
  expect(isRecord(request) && isRecord(request.params) ? request.params.session_start_source : null)
    .toBe("new");
});

test("Pi waits for a replacement session report before publishing state", async () => {
  const recordingSocketPath = join(tmpdir(), `herdr-pi-session-order-${process.pid}.sock`);
  socketPath = recordingSocketPath;
  await rm(recordingSocketPath, { force: true });

  const requests: unknown[] = [];
  let acknowledgeSessionReport: (() => void) | undefined;
  const recordingServer = createServer((socket) => {
    let input = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      input += chunk;
      const newline = input.indexOf("\n");
      if (newline === -1) {
        return;
      }
      const request = JSON.parse(input.slice(0, newline));
      requests.push(request);
      if (isRecord(request) && request.method === "pane.report_agent_session") {
        acknowledgeSessionReport = () => socket.end("{}\n");
        return;
      }
      socket.end("{}\n");
    });
  });
  server = recordingServer;
  await new Promise<void>((resolve, reject) => {
    recordingServer.once("error", reject);
    recordingServer.listen(originalPlatform === "win32" ? `\\\\.\\pipe\\${recordingSocketPath}` : recordingSocketPath, resolve);
  });

  configureIntegrationEnvironment(recordingSocketPath);
  const { handlers, pi } = createExtensionHarness();
  const { default: install } = await importFresh("./pi/herdr-agent-state.ts");
  install(pi);

  const sessionStart = handlers.get("session_start");
  expect(sessionStart).toBeDefined();
  const sessionStartResult = sessionStart?.(
    { reason: "new" },
    {
      hasUI: true,
      mode: "tui",
      isIdle: () => false,
      sessionManager: {
        getSessionFile: () => "/tmp/pi-new.jsonl",
        getSessionId: () => "pi-new",
      },
    },
  );

  const deadline = Date.now() + 1_000;
  while (Date.now() < deadline && acknowledgeSessionReport === undefined) {
    await Bun.sleep(5);
  }
  expect(acknowledgeSessionReport).toBeDefined();
  expect(
    requests.some((request) => isRecord(request) && request.method === "pane.report_agent"),
  ).toBe(false);

  acknowledgeSessionReport?.();
  await sessionStartResult;

  const stateDeadline = Date.now() + 1_000;
  while (
    Date.now() < stateDeadline &&
    !requests.some((request) => isRecord(request) && request.method === "pane.report_agent")
  ) {
    await Bun.sleep(5);
  }
  expect(requests.map((request) => (isRecord(request) ? request.method : undefined))).toEqual([
    "pane.report_agent_session",
    "pane.report_agent",
  ]);
});

async function startDroppedFirstResponseServer(name: string) {
  const recordingSocketPath = join(tmpdir(), `herdr-${name}-${process.pid}.sock`);
  socketPath = recordingSocketPath;
  await rm(recordingSocketPath, { force: true });

  let connectionCount = 0;
  const attemptedRequests: unknown[] = [];
  const deliveredRequests: unknown[] = [];
  const recordingServer = createServer((socket) => {
    connectionCount += 1;
    const connectionNumber = connectionCount;
    let input = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      input += chunk;
      const newline = input.indexOf("\n");
      if (newline === -1) {
        return;
      }
      const request = JSON.parse(input.slice(0, newline));
      attemptedRequests.push(request);
      if (connectionNumber === 1) {
        return;
      }
      deliveredRequests.push(request);
      socket.end("{}\n");
    });
  });
  server = recordingServer;
  await new Promise<void>((resolve, reject) => {
    recordingServer.once("error", reject);
    recordingServer.listen(originalPlatform === "win32" ? `\\\\.\\pipe\\${recordingSocketPath}` : recordingSocketPath, resolve);
  });

  configureIntegrationEnvironment(recordingSocketPath);
  return {
    attemptedRequests,
    deliveredRequests,
    connectionCount: () => connectionCount,
  };
}

test("Oh My Pi retries working before a queued idle state", async () => {
  const { attemptedRequests } = await startDroppedFirstResponseServer("omp-retry");
  process.env.HERDR_OMP_IDLE_DEBOUNCE_MS = "0";
  const { handlers, pi } = createExtensionHarness();

  const { default: install } = await importFresh("./omp/herdr-agent-state.ts");
  install(pi);

  const context = {
    hasUI: true,
    isIdle: () => false,
    sessionManager: {
      getSessionFile: () => undefined,
      getSessionId: () => undefined,
    },
  };
  handlers.get("session_start")?.({ reason: "startup" }, context);
  handlers.get("agent_end")?.({ messages: [] }, context);

  const deadline = Date.now() + 2_500;
  while (Date.now() < deadline && attemptedRequests.length < 3) {
    await Bun.sleep(5);
  }

  expect(attemptedRequests).toHaveLength(3);
  expect(attemptedRequests[1]).toEqual(attemptedRequests[0]);
  expect(requestState(attemptedRequests[0])).toBe("working");
  expect(requestState(attemptedRequests[2])).toBe("idle");
});

test("Oh My Pi keeps working when a turn ends with a scheduled continuation", async () => {
  const requests = await startRecordingServer("omp-will-continue");
  process.env.HERDR_OMP_IDLE_DEBOUNCE_MS = "0";
  const { handlers, pi } = createExtensionHarness();

  const { default: install } = await importFresh("./omp/herdr-agent-state.ts");
  install(pi);

  let idle = true;
  const context = {
    hasUI: true,
    isIdle: () => idle,
    sessionManager: {
      getSessionFile: () => undefined,
      getSessionId: () => undefined,
    },
  };

  handlers.get("session_start")?.({ reason: "startup" }, context);
  await waitFor(() => requestStates(requests).length === 1);

  idle = false;
  handlers.get("agent_start")?.({}, context);
  await waitFor(() => requestStates(requests).length === 2);
  expect(requestStates(requests)).toEqual(["idle", "working"]);

  // OMP already scheduled an automatic continuation, so this loop end is not a
  // user-visible settle and must not publish idle. See issue #2851.
  handlers.get("agent_end")?.({ messages: [], willContinue: true }, context);
  await Bun.sleep(50);
  expect(requestStates(requests)).toEqual(["idle", "working"]);

  // The real terminal end still settles the pane.
  idle = true;
  handlers.get("agent_end")?.({ messages: [] }, context);
  await waitFor(() => requestStates(requests).length === 3);
  expect(requestStates(requests)).toEqual(["idle", "working", "idle"]);
});

test("Pi retries working state after an unanswered socket attempt", async () => {
  const { attemptedRequests, deliveredRequests, connectionCount } =
    await startDroppedFirstResponseServer("pi-retry");
  const { handlers, pi } = createExtensionHarness();

  const { default: install } = await importFresh("./pi/herdr-agent-state.ts");
  install(pi);

  const sessionStart = handlers.get("session_start");
  expect(sessionStart).toBeDefined();
  await sessionStart?.(
    { reason: "startup" },
    {
      hasUI: true,
      mode: "tui",
      isIdle: () => false,
      sessionManager: {
        getSessionFile: () => undefined,
        getSessionId: () => undefined,
      },
    },
  );

  const reportedWorking = () =>
    deliveredRequests.some((request) => {
      if (!isRecord(request) || request.method !== "pane.report_agent") {
        return false;
      }
      const params = request.params;
      return isRecord(params) && params.state === "working";
    });

  const deadline = Date.now() + 2_500;
  while (Date.now() < deadline && !reportedWorking()) {
    await Bun.sleep(5);
  }

  expect(connectionCount()).toBeGreaterThanOrEqual(2);
  expect(attemptedRequests.length).toBeGreaterThanOrEqual(2);
  expect(attemptedRequests[1]).toEqual(attemptedRequests[0]);
  expect(reportedWorking()).toBe(true);
});

function completionHandlers(handlers: Map<string, Handler>): string[] {
  return ["agent_end", "agent_settled"].filter((event) => handlers.has(event));
}

function piContext(isIdle: () => boolean) {
  return {
    hasUI: true,
    mode: "tui",
    isIdle,
    sessionManager: {
      getSessionFile: () => undefined,
      getSessionId: () => undefined,
    },
  };
}

function requestStates(requests: unknown[]): unknown[] {
  return requests
    .filter((request) => isRecord(request) && request.method === "pane.report_agent")
    .map(requestState);
}

async function waitFor(predicate: () => boolean, timeoutMs = 1_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline && !predicate()) {
    await Bun.sleep(5);
  }
  expect(predicate()).toBe(true);
}

function requestState(request: unknown): unknown {
  if (!isRecord(request) || !isRecord(request.params)) {
    return undefined;
  }
  return request.params.state;
}

function requestSessionPath(request: unknown): unknown {
  if (!isRecord(request) || !isRecord(request.params)) {
    return undefined;
  }
  return request.params.agent_session_path;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function claudeHookCommand(action: string) {
  return originalPlatform === "win32"
    ? ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
        join(import.meta.dir, "claude/herdr-agent-state.ps1"), action]
    : ["sh", join(import.meta.dir, "claude/herdr-agent-state.sh"), action];
}

async function runClaudeHook(
  action: string,
  payload: unknown,
  environment: Record<string, string> = {},
) {
  const env = { ...process.env, ...environment };
  if (!("CURSOR_VERSION" in environment)) delete env.CURSOR_VERSION;
  const child = Bun.spawn(claudeHookCommand(action), {
    env,
    stdin: new Blob([typeof payload === "string" || payload instanceof Uint8Array ? payload : JSON.stringify(payload)]),
    stdout: "pipe",
    stderr: "pipe",
  });
  const [code, stdout, stderr] = await Promise.all([
    child.exited,
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
  ]);
  expect({ code, stdout, stderr }).toEqual({ code: 0, stdout: "", stderr: "" });
}

for (const [action, hookEvent, event] of [
  ["subagent-start", "SubagentStart", "subagent_started"],
  ["subagent-stop", "SubagentStop", "subagent_stop_observed"],
] as const) {
  const recordingName = action === "subagent-start" ? "claude-start" : "claude-stop";
  test(`Claude ${hookEvent} reports only structured subagent observations`, async () => {
    const requests = await startRecordingServer(recordingName);
    await runClaudeHook(action, {
      hook_event_name: hookEvent,
      session_id: "root-session",
      agent_id: "child-123",
      agent_type: "Explore λ",
      stop_hook_active: true,
      transcript_path: "/does/not/exist/root.jsonl",
      agent_transcript_path: "/does/not/exist/child.jsonl",
      last_assistant_message: "Do not capture this message",
      background_tasks: [{ id: "shell-123", status: "running" }],
    });
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({
      method: "background.report",
      params: {
        pane_id: "test:p1",
        source: "herdr:claude",
        session_id: "root-session",
        event,
        task_id: "child-123",
        task_type: "Explore λ",
      },
    });
    expect(Object.keys((requests[0] as any).params).sort()).toEqual(
      ["pane_id", "source", "session_id", "event", "task_id", "task_type"].sort(),
    );
  });

  test(`Claude ${hookEvent} omits absent or invalid optional agent types`, async () => {
    const requests = await startRecordingServer(`${recordingName}-opt`);
    for (const agent_type of [undefined, "", " ", 123, {}]) {
      await runClaudeHook(action, {
        hook_event_name: hookEvent,
        session_id: "root-session",
        agent_id: "child-123",
        agent_type,
      });
    }
    expect(requests).toHaveLength(5);
    for (const request of requests as any[]) {
      expect(request.method).toBe("background.report");
      expect(request.params).not.toHaveProperty("task_type");
    }
  }, 30_000);

  test(`Claude ${hookEvent} ignores malformed identity and mismatched events`, async () => {
    const requests = await startRecordingServer(`${recordingName}-bad`);
    const valid = { hook_event_name: hookEvent, session_id: "root-session", agent_id: "child-123" };
    for (const payload of [
      "{", [], null, 123,
      { ...valid, session_id: undefined },
      { ...valid, session_id: 123 },
      { ...valid, session_id: "" },
      { ...valid, session_id: " " },
      { ...valid, agent_id: undefined },
      { ...valid, agent_id: " " },
      { ...valid, agent_id: {} },
      { ...valid, hook_event_name: "SessionStart" },
      { ...valid, hook_event_name: hookEvent === "SubagentStart" ? "SubagentStop" : "SubagentStart" },
      { ...valid, cursor_version: "compatibility" },
    ]) {
      await runClaudeHook(action, payload);
    }
    await runClaudeHook(action, valid, { CURSOR_VERSION: "compatibility" });
    await runClaudeHook(action, valid, { HERDR_ENV: "0" });
    await runClaudeHook(action, valid, { HERDR_PANE_ID: "" });
    await runClaudeHook(action, valid, { HERDR_SOCKET_PATH: "" });
    expect(requests).toEqual([]);
  }, 30_000);
}

test.skipIf(originalPlatform === "win32")("Claude SessionStart preserves root identity and source without reporting state", async () => {
  const requests = await startRecordingServer("claude-root-session");
  for (const source of ["startup", "resume", "clear", "compact", "fork"]) {
    await runClaudeHook("session", {
      hook_event_name: "SessionStart",
      session_id: "root-session",
      transcript_path: "/unread/한국어/session.jsonl",
      source,
      agent_type: "custom-root-agent",
    });
  }
  expect(requests).toHaveLength(5);
  expect((requests as any[]).map(request => request.params.session_start_source))
    .toEqual(["startup", "resume", "clear", "compact", "fork"]);
  for (const request of requests as any[]) {
    expect(request.method).toBe("pane.report_agent_session");
    expect(request.params).toMatchObject({
      agent: "claude", source: "herdr:claude", pane_id: "test:p1",
      agent_session_id: "root-session", agent_session_path: "/unread/한국어/session.jsonl",
    });
    expect(request.params).not.toHaveProperty("state");
    expect(typeof request.params.seq).toBe("number");
  }
  for (const event of ["SessionStart", "SubagentStart", "SubagentStop"]) {
    await runClaudeHook("session", {
      hook_event_name: event, session_id: "root-session", agent_id: "child-123",
    });
  }
  expect(requests).toHaveLength(5);
});

for (const [action, hook_event_name] of [
  ["stop", "Stop"],
  ["subagent-stop", "SubagentStop"],
] as const) {
  test(`Claude ${hook_event_name} maps only native shell snapshot fields`, async () => {
    const requests = await startRecordingServer(`claude-${action}-shell`);
    const payload = {
      hook_event_name,
      session_id: "root-session",
      agent_id: "reporter-λ",
      background_tasks: [
        { id: "shell-1", type: "shell", status: "native-future-status", command: "echo 한국어\nprintf λ", description: "private description", output_file: "/unread/output" },
        { id: "shell-2", type: "shell", status: "pending" },
        { id: "replacement", type: "shell", status: "running", command: "printf \ufffd" },
        { id: "surrogate-pair", type: "shell", status: "running", command: "printf 😀" },
        { id: "literal-escapes", type: "shell", status: "running", command: "printf \\ud800 \\udc00" },
        { id: "not-shell", type: "local_bash", status: "running" },
        { id: "wrong-case", type: "Shell", status: "running" },
        { id: "agent", type: "agent", status: "running" },
      ],
      last_assistant_message: "private message",
      agent_transcript_path: "/unread/transcript",
    };
    // Exercise the pair as original JSON escapes rather than a literal emoji.
    await runClaudeHook(action, JSON.stringify(payload).replace("😀", "\\ud83d\\ude00"));
    expect((requests as any[]).filter(request => request.method === "background.shell.report")).toEqual([{
      id: expect.any(String),
      method: "background.shell.report",
      params: {
        pane_id: "test:p1",
        source: "herdr:claude",
        session_id: "root-session",
        observation: {
          event: "snapshot_observed",
          hook_event: hook_event_name,
          hook_agent_id: "reporter-λ",
          tasks: [
            { task_id: "shell-1", reported_status: "native-future-status", command: "echo 한국어\nprintf λ" },
            { task_id: "shell-2", reported_status: "pending" },
            { task_id: "replacement", reported_status: "running", command: "printf \ufffd" },
            { task_id: "surrogate-pair", reported_status: "running", command: "printf 😀" },
            { task_id: "literal-escapes", reported_status: "running", command: "printf \\ud800 \\udc00" },
          ],
          truncated: false,
        },
      },
    }]);
    expect(requests).toHaveLength(action === "subagent-stop" ? 2 : 1);
    if (action === "subagent-stop") expect((requests[0] as any).method).toBe("background.report");
  });

  test(`Claude ${hook_event_name} preserves lifecycle when optional snapshots are absent or malformed`, async () => {
    const requests = await startRecordingServer(`claude-${action}-malformed`);
    const valid = { id: "shell-1", type: "shell", status: "running" };
    const retained = Array.from({ length: 128 }, (_, id) => ({ ...valid, id: `shell-${id}` }));
    const snapshots = [
      undefined, null, {}, "[]", [null], [{}],
      [{ ...valid, id: "" }], [{ ...valid, id: "x\u001b" }],
      [{ ...valid, id: "λ".repeat(129) }],
      [{ ...valid, status: 42 }], [{ ...valid, status: "native\nstatus" }],
      [{ ...valid, status: "λ".repeat(65) }],
      [{ ...valid, command: {} }], [valid, {}],
      [...retained, { ...valid, type: null }],
      [...retained, { ...valid, id: "" }],
      [...retained, { ...valid, status: "bad\nstatus" }],
      [...retained, { ...valid, command: {} }],
      [...retained, { ...valid, command: "\ud800" }],
      [{ ...valid, command: "\ud800" }],
      [{ ...valid, command: "\udc00" }],
      [...retained, { ...valid, command: "\udc00" }],
    ];
    for (const [index, background_tasks] of snapshots.entries()) {
      const before = requests.length;
      await runClaudeHook(action, {
        hook_event_name, session_id: "root-session", agent_id: "reporter",
        background_tasks,
      });
      expect({
        fixture: index + 1,
        methods: requests.slice(before).map((request: any) => request.method),
      }).toEqual({
        fixture: index + 1,
        methods: action === "subagent-stop" ? ["background.report"] : [],
      });
    }
    expect(requests).toHaveLength(action === "subagent-stop" ? snapshots.length : 0);
    for (const request of requests as any[]) expect(request.method).toBe("background.report");
  }, 30_000);

  test(`Claude ${hook_event_name} preserves real empty snapshots and discloses bounded rows and commands`, async () => {
    const requests = await startRecordingServer(`claude-${action}-bounds`);
    const base = { hook_event_name, session_id: "root-session", agent_id: "reporter" };
    await runClaudeHook(action, { ...base, background_tasks: [] });
    await runClaudeHook(action, {
      ...base,
      background_tasks: [{ id: "shell-long", type: "shell", status: "running", command: "λ😀\n".repeat(800) }],
    });
    await runClaudeHook(action, {
      ...base,
      background_tasks: Array.from({ length: 129 }, (_, id) => ({
        id: `shell-${id}`, type: "shell", status: "running",
        command: "x".repeat(4096),
      })),
    });
    const observations = (requests as any[])
      .filter(request => request.method === "background.shell.report")
      .map(request => request.params.observation);
    expect(observations).toHaveLength(3);
    expect(observations[0]).toMatchObject({ tasks: [], truncated: false });
    expect(observations[1].tasks).toHaveLength(1);
    expect(observations[1].truncated).toBe(true);
    const command = observations[1].tasks[0].command;
    expect(Buffer.byteLength(command, "utf8")).toBeLessThanOrEqual(4096);
    expect(command.endsWith("\n[truncated]")).toBe(true);
    expect(command).not.toContain("�");
    expect(observations[2].tasks).toHaveLength(128);
    expect(observations[2].truncated).toBe(true);
    expect(observations[2].tasks[0].command).toBe("x".repeat(4096));
  });

  test(`Claude ${hook_event_name} retains 100 Unicode commands within the UTF-8 wire cap`, async () => {
    const wireSizes: number[] = [];
    const requests = await startRecordingServer(`claude-${action}-unicode-wire`, wireSizes);
    const command = "λ".repeat(2048);
    const background_tasks = Array.from({ length: 100 }, (_, id) => ({
      id: `shell-${id}`, type: "shell", status: "running", command,
    }));
    await runClaudeHook(action, {
      hook_event_name, session_id: "root-session", agent_id: "reporter", background_tasks,
    });
    expect(requests).toHaveLength(action === "subagent-stop" ? 2 : 1);
    const report = (requests as any[]).find(request => request.method === "background.shell.report");
    expect(report.params.observation.tasks).toHaveLength(100);
    expect(report.params.observation).toMatchObject({
      tasks: background_tasks.map(task => ({ task_id: task.id, reported_status: task.status, command })),
      truncated: false,
    });
    expect(wireSizes).toHaveLength(requests.length);
    for (const size of wireSizes) expect(size).toBeLessThanOrEqual(1048576);
  });

  test(`Claude ${hook_event_name} trims tail rows when report overhead crosses the wire cap`, async () => {
    const wireSizes: number[] = [];
    const requests = await startRecordingServer(`claude-${action}-wire-limit`, wireSizes);
    const background_tasks = Array.from({ length: 128 }, (_, id) => ({
      id: `shell-${id}`, type: "shell", status: "running", command: '"'.repeat(4000),
    }));
    const payload = { hook_event_name, session_id: "root-session", agent_id: "reporter", background_tasks };
    // Leave less room than the report envelope needs, without clipping any command.
    let remaining = 1048576 - 16 - Buffer.byteLength(JSON.stringify(payload), "utf8");
    for (const task of background_tasks) {
      const extra = Math.min(4096 - task.command.length, Math.floor(remaining / 2));
      task.command += '"'.repeat(extra);
      remaining -= extra * 2;
    }
    const input = JSON.stringify(payload);
    expect(Buffer.byteLength(input, "utf8")).toBeGreaterThanOrEqual(1048576 - 17);
    expect(Buffer.byteLength(input, "utf8")).toBeLessThanOrEqual(1048576);
    await runClaudeHook(action, input);
    expect(requests).toHaveLength(action === "subagent-stop" ? 2 : 1);
    if (action === "subagent-stop") expect((requests[0] as any).method).toBe("background.report");
    const report = (requests as any[]).find(request => request.method === "background.shell.report");
    const observation = report.params.observation;
    const tasks = background_tasks.map(task => ({
      task_id: task.id, reported_status: task.status, command: task.command,
    }));
    const untrimmedReport = {
      ...report,
      params: { ...report.params, observation: { ...observation, tasks, truncated: false } },
    };
    expect(Buffer.byteLength(JSON.stringify(untrimmedReport) + "\n", "utf8")).toBeGreaterThan(1048576);
    expect(observation.truncated).toBe(true);
    expect(observation.tasks.length).toBeGreaterThan(0);
    expect(observation.tasks.length).toBeLessThan(128);
    expect(observation.tasks).toEqual(tasks.slice(0, observation.tasks.length));
    expect(wireSizes).toHaveLength(requests.length);
    for (const size of wireSizes) expect(size).toBeLessThanOrEqual(1048576);
  });
}

test("Claude snapshot bounds 10000 validated native rows to the first 128", async () => {
  const requests = await startRecordingServer("claude-many-native-rows");
  const background_tasks = Array.from({ length: 10000 }, (_, id) => ({
    id: `shell-${id}`, type: "shell", status: "running", command: "λ",
  }));
  const payload = { hook_event_name: "Stop", session_id: "root-session", background_tasks };
  expect(Buffer.byteLength(JSON.stringify(payload), "utf8")).toBeLessThanOrEqual(1048576);
  await runClaudeHook("stop", payload);
  expect(requests).toHaveLength(1);
  expect((requests[0] as any).params.observation.tasks).toHaveLength(128);
  expect((requests[0] as any).params.observation).toMatchObject({
    tasks: background_tasks.slice(0, 128).map(task => ({
      task_id: task.id, reported_status: task.status, command: task.command,
    })),
    truncated: true,
  });
}, 30_000);

test("Claude Stop permits absent reporter provenance and requires exact native event data", async () => {
  const requests = await startRecordingServer("claude-stop-provenance");
  const valid = { hook_event_name: "Stop", session_id: "root-session", background_tasks: [] };
  for (const payload of [
    [valid],
    { ...valid, hook_event_name: "SubagentStop" },
    { ...valid, hook_event_name: "stop" },
    { ...valid, agent_id: null }, { ...valid, agent_id: "" },
    { ...valid, agent_id: "reporter\u0085" },
    { ...valid, session_id: "session\u0000" },
    { ...valid, session_id: "λ".repeat(129) },
  ]) await runClaudeHook("stop", payload);
  await runClaudeHook("subagent-stop", { ...valid, hook_event_name: "SubagentStop" });
  expect(requests).toEqual([]);
  await runClaudeHook("stop", valid);
  expect(requests).toHaveLength(1);
  expect((requests[0] as any).params.observation).toEqual({
    event: "snapshot_observed", hook_event: "Stop", tasks: [], truncated: false,
  });
}, 30_000);

const bashObservationPayload = {
  hook_event_name: "PostToolUse",
  session_id: "root-session",
  tool_name: "Bash",
  tool_use_id: "tool-λ",
  tool_response: { backgroundTaskId: "shell-λ", stdout: "private output", stderr: "private error" },
  tool_input: { command: "echo λ\nprintf 한국어", description: "private description" },
};

test("Claude Bash observes structured automatic backgrounding without intent flags", async () => {
  const requests = await startRecordingServer("claude-bash-native");
  await runClaudeHook("bash-background", bashObservationPayload);
  await runClaudeHook("bash-background", {
    ...bashObservationPayload, agent_id: "reporter",
    tool_input: { run_in_background: false },
  });
  expect(requests).toEqual([
    {
      id: expect.any(String), method: "background.shell.report",
      params: {
        pane_id: "test:p1", source: "herdr:claude", session_id: "root-session",
        observation: {
          event: "bash_background_observed", task_id: "shell-λ", tool_use_id: "tool-λ",
          command: "echo λ\nprintf 한국어",
        },
      },
    },
    {
      id: expect.any(String), method: "background.shell.report",
      params: {
        pane_id: "test:p1", source: "herdr:claude", session_id: "root-session",
        observation: {
          event: "bash_background_observed", task_id: "shell-λ", tool_use_id: "tool-λ",
          hook_agent_id: "reporter",
        },
      },
    },
  ]);
});

test("Claude Bash requires exact event, tool, response key and bounded native identities", async () => {
  const requests = await startRecordingServer("claude-bash-invalid");
  for (const payload of [
    { ...bashObservationPayload, hook_event_name: "PostToolUseFailure" },
    { ...bashObservationPayload, tool_name: "bash" },
    { ...bashObservationPayload, tool_name: "BashOther" },
    { ...bashObservationPayload, tool_name: "Shell" },
    { ...bashObservationPayload, tool_name: ["Bash"] },
    { ...bashObservationPayload, tool_response: { backgroundtaskid: "wrong-case" } },
    { ...bashObservationPayload, tool_response: "backgroundTaskId: shell-λ" },
    { ...bashObservationPayload, tool_response: { backgroundTaskId: 1 } },
    { ...bashObservationPayload, tool_response: { backgroundTaskId: "" } },
    { ...bashObservationPayload, tool_response: { backgroundTaskId: "id\n" } },
    { ...bashObservationPayload, tool_response: { backgroundTaskId: "λ".repeat(129) } },
    { ...bashObservationPayload, tool_response: {}, tool_input: { run_in_background: true } },
    { ...bashObservationPayload, tool_use_id: undefined },
    { ...bashObservationPayload, tool_use_id: "id\u007f" },
    { ...bashObservationPayload, tool_use_id: "x".repeat(257) },
    { ...bashObservationPayload, agent_id: "reporter\t" },
    { ...bashObservationPayload, tool_input: { command: {} } },
  ]) await runClaudeHook("bash-background", payload);
  expect(requests).toEqual([]);
}, 30_000);

test("Claude Bash clips UTF-8 commands with an explicit truncation marker", async () => {
  const requests = await startRecordingServer("claude-bash-truncate");
  await runClaudeHook("bash-background", {
    ...bashObservationPayload, tool_input: { command: "λ😀\n".repeat(800) },
  });
  expect(requests).toHaveLength(1);
  const observation = (requests[0] as any).params.observation;
  expect(Buffer.byteLength(observation.command, "utf8")).toBeLessThanOrEqual(4096);
  expect(observation.command.endsWith("\n[truncated]")).toBe(true);
  expect(observation.command).not.toContain("�");
  expect(observation).not.toHaveProperty("truncated");
});

test("Claude hook rejects oversized raw bytes before JSON parsing and accepts the exact cap", async () => {
  const requests = await startRecordingServer("claude-input-limit");
  const payload = { hook_event_name: "Stop", session_id: "root-session", background_tasks: [] };
  const valid = JSON.stringify(payload);
  await runClaudeHook("stop", valid.padEnd(1048577));
  await runClaudeHook("stop", { ...payload, ignored: "λ".repeat(550_000) });
  await runClaudeHook("stop", new Uint8Array([0xff, 0xfe]));
  expect(requests).toEqual([]);
  await runClaudeHook("stop", valid.padEnd(1048576));
  expect(requests).toHaveLength(1);
});

test("Claude hook returns after cap+1 bytes without waiting for stdin EOF", async () => {
  const requests = await startRecordingServer("claude-input-open");
  const env = { ...process.env };
  delete env.CURSOR_VERSION;
  const child = Bun.spawn(claudeHookCommand("stop"), {
    env, stdin: "pipe", stdout: "pipe", stderr: "pipe",
  });
  try {
    child.stdin.write(new Uint8Array(1048577).fill(32));
    expect(await Promise.race([
      child.exited,
      Bun.sleep(8_000).then(() => { throw new Error("hook waited for EOF beyond raw input cap"); }),
    ])).toBe(0);
    expect(await new Response(child.stdout).text()).toBe("");
    expect(await new Response(child.stderr).text()).toBe("");
    expect(requests).toEqual([]);
  } finally {
    child.stdin.end();
    child.kill();
  }
}, 15_000);
