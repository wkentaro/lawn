use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ratatui::layout::Rect;
use tokio::sync::{mpsc, Notify};

use crate::api::schema::{Method, PaneTarget, Request, ResponseResult};
use crate::events::AppEvent;
use crate::layout::PaneId;
use crate::protocol::{FrameData, PaneSurfaceFrame};
use crate::render_signal::RenderSignal;
use crate::terminal::TerminalRuntime;

use super::events::ClientLoopEvent;

const CORE_RENDER_ENV: &str = "LAWN_CORE_RENDER";

const OBSERVER_PANE_ID: u32 = u32::MAX;

const OUTPUT_RETRY: std::time::Duration = std::time::Duration::from_millis(16);

pub(crate) struct CorePane {
    pane_id: String,
    runtime: TerminalRuntime,
    ended: Arc<AtomicBool>,
    changed: Arc<AtomicBool>,
    wake: tokio::task::JoinHandle<()>,
    events: tokio::task::JoinHandle<()>,
}

impl Drop for CorePane {
    fn drop(&mut self) {
        self.wake.abort();
        self.events.abort();
    }
}

impl CorePane {
    pub(super) fn observe(
        pane_id: String,
        address: &str,
        size: (u16, u16),
        loop_events: mpsc::Sender<ClientLoopEvent>,
        output_pending: Arc<AtomicBool>,
    ) -> Option<Self> {
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(RenderSignal::new());
        let (app_events, mut app_events_rx) = mpsc::channel(64);
        let (cols, rows) = size;
        let runtime = match TerminalRuntime::observe_core(
            PaneId::from_raw(OBSERVER_PANE_ID),
            address,
            rows,
            cols,
            0,
            app_events,
            render_notify.clone(),
            render_dirty.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::warn!(%pane_id, %address, %error, "cannot observe the core terminal");
                return None;
            }
        };
        tracing::info!(%pane_id, %address, "rendering the pane from herdr-core");
        let changed = Arc::new(AtomicBool::new(false));
        let wake = {
            let changed = changed.clone();
            let loop_events = loop_events.clone();
            tokio::spawn(async move {
                loop {
                    render_notify.notified().await;
                    let _ = render_dirty.take();
                    changed.store(true, Ordering::Release);
                    if output_pending.swap(true, Ordering::AcqRel) {
                        continue;
                    }
                    match loop_events.try_send(ClientLoopEvent::CorePaneOutput) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            output_pending.store(false, Ordering::Release);
                            tokio::time::sleep(OUTPUT_RETRY).await;
                            render_notify.notify_one();
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => return,
                    }
                }
            })
        };
        let ended = Arc::new(AtomicBool::new(false));
        let events = {
            let ended = ended.clone();
            let pane_id = pane_id.clone();
            tokio::spawn(async move {
                while let Some(event) = app_events_rx.recv().await {
                    if matches!(event, AppEvent::PaneDied { .. }) {
                        ended.store(true, Ordering::Release);
                        let _ = loop_events
                            .send(ClientLoopEvent::CorePaneEnded { pane_id })
                            .await;
                        return;
                    }
                }
            })
        };
        Some(Self {
            pane_id,
            runtime,
            ended,
            changed,
            wake,
            events,
        })
    }

    pub(crate) fn take_change(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn target(&self, surface: &PaneSurfaceFrame) -> Option<Rect> {
        let pane = surface
            .panes
            .iter()
            .find(|pane| pane.pane_id == self.pane_id)?;
        let scrolled_back = pane
            .scroll
            .is_some_and(|scroll| scroll.offset_from_bottom > 0);
        let inner = Rect::new(
            pane.inner_rect.x,
            pane.inner_rect.y,
            pane.inner_rect.width,
            pane.inner_rect.height,
        );
        let drawable = !scrolled_back
            && !self.ended.load(Ordering::Acquire)
            && !self.runtime.synchronized_output_active()
            && self.runtime.terminal_dimensions() == Some((inner.width, inner.height));
        drawable.then_some(inner)
    }

    pub(crate) fn render(&self, surface: &PaneSurfaceFrame) -> Option<(Rect, FrameData)> {
        let inner = self.target(surface)?;
        let area = Rect::new(0, 0, inner.width, inner.height);
        let synchronized = self.runtime.synchronized_output_state();
        let (buffer, _) = crate::ui::render_terminal_virtual(&self.runtime, area);
        if synchronized.0 || self.runtime.synchronized_output_state() != synchronized {
            return None;
        }
        let hyperlinks = self.runtime.visible_hyperlinks(area);
        Some((
            inner,
            FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &hyperlinks),
        ))
    }
}

const LOOKUP_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

struct Lookup {
    generation: u64,
    retry_at: Option<std::time::Instant>,
}

pub(super) struct CoreRender {
    loop_events: mpsc::Sender<ClientLoopEvent>,
    output_pending: Arc<AtomicBool>,
    requested: HashMap<String, Lookup>,
    next_generation: u64,
}

impl CoreRender {
    pub(super) fn from_env(loop_events: mpsc::Sender<ClientLoopEvent>) -> Option<Self> {
        (std::env::var_os(CORE_RENDER_ENV).is_some_and(|value| value == "1")).then(|| Self {
            loop_events,
            output_pending: Arc::new(AtomicBool::new(false)),
            requested: HashMap::new(),
            next_generation: 0,
        })
    }

    pub(super) fn follow(&mut self, panes: &[(&str, (u16, u16))]) {
        self.requested
            .retain(|requested, _| panes.iter().any(|(pane_id, _)| pane_id == requested));
        let now = std::time::Instant::now();
        for &(pane_id, size) in panes {
            let due = self
                .requested
                .get(pane_id)
                .is_none_or(|lookup| lookup.retry_at.is_some_and(|retry_at| now >= retry_at));
            if !due {
                continue;
            }
            self.next_generation += 1;
            let generation = self.next_generation;
            self.requested.insert(
                pane_id.to_owned(),
                Lookup {
                    generation,
                    retry_at: None,
                },
            );
            let pane_id = pane_id.to_owned();
            let loop_events = self.loop_events.clone();
            let output_pending = self.output_pending.clone();
            tokio::task::spawn_blocking(move || {
                let pane = core_terminal_of(&pane_id).and_then(|address| {
                    CorePane::observe(
                        pane_id.clone(),
                        &address,
                        size,
                        loop_events.clone(),
                        output_pending,
                    )
                });
                let _ = loop_events.blocking_send(ClientLoopEvent::CorePaneFound {
                    pane_id,
                    generation,
                    pane: pane.map(Box::new),
                });
            });
        }
    }

    pub(super) fn accept(&self, pane_id: &str, generation: u64) -> bool {
        self.requested
            .get(pane_id)
            .is_some_and(|lookup| lookup.generation == generation)
    }

    pub(super) fn wants(&self, pane_id: &str) -> bool {
        self.requested.contains_key(pane_id)
    }

    pub(super) fn retry_later(&mut self, pane_id: &str, generation: u64) {
        if let Some(lookup) = self
            .requested
            .get_mut(pane_id)
            .filter(|lookup| lookup.generation == generation)
        {
            lookup.retry_at = Some(std::time::Instant::now() + LOOKUP_RETRY);
        }
    }

    pub(super) fn forget(&mut self, pane_id: &str) {
        self.requested.remove(pane_id);
    }

    pub(super) fn take_output(&self) {
        self.output_pending.store(false, Ordering::Release);
    }
}

fn core_terminal_of(pane_id: &str) -> Option<String> {
    let response = crate::api::client::ApiClient::local()
        .request(Request {
            id: "core-render".into(),
            method: Method::PaneGet(PaneTarget {
                pane_id: pane_id.to_owned(),
            }),
        })
        .inspect_err(
            |error| tracing::warn!(%pane_id, %error, "cannot look up the pane's core terminal"),
        )
        .ok()?;
    match response.result {
        ResponseResult::PaneInfo { pane } => pane.core_terminal,
        _ => None,
    }
}
