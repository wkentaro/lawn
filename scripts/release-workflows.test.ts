// Modified for Lawn.
import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

const load = (name: string): any =>
  Bun.YAML.parse(readFileSync(new URL(`../.github/workflows/${name}.yml`, import.meta.url), "utf8"));
const preview = load("preview");
const release = load("release");
const lawn = load("lawn-release");
const adminGate = release.jobs["validate-release-source"].steps[0];

describe("official publishing workflow boundaries", () => {
  test("publishing is tag-only while normal PR CI remains enabled", () => {
    expect(preview.on).toEqual({ push: { tags: ["preview-*"] } });
    expect(release.on).toEqual({ push: { tags: ["v*"] } });
    expect(load("ci").on.pull_request).toBeDefined();
  });

  test("preview checks do not require a workstation Windows SDK", () => {
    const checks = preview.jobs.preflight.steps.find((step: any) => step.name === "Run checks");
    expect(checks.run.trim().split("\n")).toEqual(["just ci", "just docs-contract-test"]);
    expect(preview.jobs.build.strategy.matrix.include).toContainEqual({
      target: "x86_64-pc-windows-msvc",
      os: "windows-latest",
      name: "herdr-windows-x86_64.zip",
    });
    expect(preview.jobs.publish.needs).toContain("build");
  });

  test("Lawn only publishes checked tags from main with a separate write job", () => {
    expect(lawn.permissions.contents).toBe("read");
    expect(lawn.on.push.tags).toEqual(["lawn-v*"]);
    expect(lawn.on.pull_request).toBeDefined();
    expect(lawn.jobs.publish.if).toContain("github.repository == 'wkentaro/lawn'");
    expect(lawn.jobs.publish.if).toContain("github.event_name == 'push'");
    expect(lawn.jobs.publish.if).toContain("startsWith(github.ref, 'refs/tags/lawn-v')");
    expect(lawn.jobs.publish.needs).toBe("assemble");
    expect(lawn.jobs.assemble.needs).toBe("build");
    expect(lawn.jobs.publish.permissions.contents).toBe("write");
    const verify = lawn.jobs.publish.steps.find((step: any) => step.name?.startsWith("Verify tag"));
    expect(verify.run).toContain('git merge-base --is-ancestor "$GITHUB_SHA" origin/main');
    expect(verify.run).toContain('refs/tags/$GITHUB_REF_NAME^{commit}');
    const checkout = lawn.jobs.publish.steps.find((step: any) => step.uses?.startsWith("actions/checkout@"));
    expect(checkout.with.ref).toBe("${{ github.sha }}");
    expect(checkout.with["persist-credentials"]).toBe(false);
  });

  test("Lawn builds all remote assets on native runners and assembles before publishing", () => {
    expect(lawn.jobs.build.strategy.matrix.include).toEqual([
      { platform: "macos-aarch64", target: "aarch64-apple-darwin", os: "macos-15" },
      { platform: "macos-x86_64", target: "x86_64-apple-darwin", os: "macos-15-intel" },
      { platform: "linux-aarch64", target: "aarch64-unknown-linux-musl", os: "ubuntu-24.04-arm" },
      { platform: "linux-x86_64", target: "x86_64-unknown-linux-musl", os: "ubuntu-24.04" },
      { platform: "windows-x86_64", target: "x86_64-pc-windows-msvc", os: "windows-2022" },
    ]);
    const packageStep = lawn.jobs.build.steps.find((step: any) => step.name === "Package and smoke");
    expect(packageStep.run).toContain('scripts/lawn_release.py --binary "$binary"');
    expect(packageStep.run).toContain('"$binary" --help');
    expect(lawn.jobs.assemble.steps.find((step: any) => step.name === "Assemble complete release")?.run)
      .toContain("scripts/lawn_release.py --inputs inputs");
    expect(lawn.jobs.assemble.if).toContain("github.repository == 'wkentaro/lawn'");
    expect(lawn.jobs.assemble.steps.find((step: any) => step.name === "Assemble complete release")?.run)
      .toContain('if [ "$IS_RELEASE" != true ]');
  });

  test("each publishing job rechecks both actors before using credentials", () => {
    for (const [workflow, names] of [
      [preview, ["preflight", "publish"]],
      [release, ["validate-release-source", "release", "update-nix-package", "close-released-issues", "update-latest-json"]],
    ] as const) {
      for (const name of names) {
        const job = workflow.jobs[name];
        expect(job.if).toContain("github.event_name == 'push'");
        expect(job.if).toContain("startsWith(github.ref, 'refs/tags/");
        expect(job.steps[0]).toEqual(adminGate);
      }
    }
    expect(adminGate.run).toContain('"$GITHUB_ACTOR" "$GITHUB_TRIGGERING_ACTOR"');
    expect(adminGate.env.GH_TOKEN).toBe("${{ github.token }}");
    expect(adminGate.run).not.toContain("ogulcancelik");
  });

  test("release arguments are not interpolated into executable shell text", () => {
    const input = `untrusted'\"$(echo unexpected-command)`;
    for (const args of [
      ["preview", input],
      ["release-prepare", input, input],
      ["release-publish", input, input],
      ["release", input, input],
    ]) {
      const result = spawnSync("just", ["--dry-run", ...args], { encoding: "utf8" });
      expect(result.status).toBe(0);
      expect(result.stdout + result.stderr).not.toContain(input);
      expect(result.stdout + result.stderr).not.toContain("unexpected-command");
    }
  });

  test.skipIf(process.platform === "win32")("admin gate permits admins and fails closed for other roles or API errors", () => {
    const dir = mkdtempSync("/var/tmp/herdr-admin-gate-");
    try {
      writeFileSync(join(dir, "gh"), `#!/bin/sh
case "$2" in
  */collaborators/admin-*/permission) echo admin ;;
  */collaborators/maintainer/permission) echo maintain ;;
  */collaborators/writer/permission) echo write ;;
  *) exit 1 ;;
esac
`, { mode: 0o755 });
      for (const [actor, trigger, succeeds] of [
        ["admin-one", "admin-two", true],
        ["writer", "admin-two", false],
        ["admin-one", "writer", false],
        ["admin-one", "maintainer", false],
        ["admin-one", "api-error", false],
      ] as const) {
        for (const gate of [adminGate, lawn.jobs.publish.steps[0]]) {
          const result = spawnSync("bash", ["-c", gate.run], {
            env: { ...process.env, PATH: `${dir}:${process.env.PATH}`, GITHUB_REPOSITORY: "example/test", GITHUB_ACTOR: actor, GITHUB_TRIGGERING_ACTOR: trigger },
            encoding: "utf8",
          });
          expect(result.status === 0).toBe(succeeds);
        }
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
