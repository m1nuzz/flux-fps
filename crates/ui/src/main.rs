//! flux-fps resident application: MVU frontend on windows-reactor.
//!
//! Bridge design (RESEARCH_FINDINGS.md 5.3 adapted to the reactor API):
//! - the Core Engine runs on a dedicated thread with its own tokio runtime;
//! - UI -> Engine commands go through a bounded tokio mpsc (`try_send` only);
//! - Engine -> UI telemetry uses a self-perpetuating `spawn_background` poll:
//!   each poll sleeps ~33 ms, clones the latest snapshot from the engine's
//!   `tokio::sync::watch` and returns it as a Message, and `update()`
//!   immediately re-arms the next poll. This respects the reactor rule that
//!   `LocalSender` is not `Send` while keeping updates at ~30 Hz.

use common::{Command, Config, TelemetrySnapshot};
use tokio::sync::{mpsc, watch};
use windows_reactor::{App, Button, Component, ComponentContext, StackPanel, TextBlock, View};

/// Engine-side handles handed to the root component as its Input.
pub struct EngineHandles {
    pub commands: mpsc::Sender<Command>,
    pub telemetry: watch::Receiver<TelemetrySnapshot>,
}

/// Messages delivered to the root component's update loop.
#[derive(Clone)]
pub enum Message {
    /// Latest telemetry snapshot from the engine (30 Hz poll cycle).
    Telemetry(TelemetrySnapshot),
    /// User asked for an immediate standby purge.
    PurgeStandbyNow,
}

pub struct FluxFpsApp {
    commands: mpsc::Sender<Command>,
    /// Engine-side watch receiver used to re-arm each telemetry poll.
    telemetry: watch::Receiver<TelemetrySnapshot>,
    latest: TelemetrySnapshot,
}

impl Component for FluxFpsApp {
    type Input = EngineHandles;
    type Message = Message;

    fn create(input: &EngineHandles, context: &ComponentContext<Self>) -> Self {
        spawn_telemetry_poll(input.telemetry.clone(), context);
        Self {
            commands: input.commands.clone(),
            telemetry: input.telemetry.clone(),
            latest: TelemetrySnapshot::default(),
        }
    }

    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        match message {
            Message::Telemetry(snapshot) => {
                self.latest = snapshot;
                // Re-arm the poll loop (self-perpetuating 30 Hz cycle).
                spawn_telemetry_poll(self.telemetry.clone(), context);
            }
            Message::PurgeStandbyNow => {
                // Bounded channel: UI must use try_send only, never block.
                let _ = self.commands.try_send(Command::PurgeStandbyNow);
            }
        }
    }

    fn view(&self, _input: &Self::Input, context: &mut windows_reactor::ViewContext<Self>) -> View {
        let dpc_peak = self
            .latest
            .dpc_max_us
            .iter()
            .copied()
            .fold(0.0f32, f32::max);
        StackPanel::new()
            .spacing(8.0)
            .children((
                TextBlock::new().text(format!("flux-fps v{}", common::VERSION)),
                TextBlock::new().text(format!(
                    "Memory: commit {:.0}%, available {} MB",
                    self.latest.commit_percent, self.latest.available_mb
                )),
                TextBlock::new().text(format!(
                    "Game Mode: {} background processes managed",
                    self.latest.managed_pids.len()
                )),
                TextBlock::new().text(format!("Peak DPC: {dpc_peak:.1} us")),
                TextBlock::new().text(format!("FPS cap: {:?}", self.latest.fps_cap)),
                Button::new()
                    .on_click(context.sender().message(Message::PurgeStandbyNow))
                    .content("Purge standby list now"),
            ))
            .into()
    }
}

/// Arms one telemetry poll: sleeps ~33 ms on the Windows thread pool, clones
/// the latest engine snapshot and delivers it as a Message. `update()` re-arms.
fn spawn_telemetry_poll(
    telemetry: watch::Receiver<TelemetrySnapshot>,
    context: &ComponentContext<FluxFpsApp>,
) {
    context.spawn_background(move |_cancellation| {
        std::thread::sleep(std::time::Duration::from_millis(33));
        Message::Telemetry(telemetry.borrow().clone())
    });
}

fn main() {
    println!("flux-fps v{} starting", common::VERSION);

    let (commands, command_rx) = mpsc::channel::<Command>(16);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<watch::Receiver<TelemetrySnapshot>>();

    // The Core Engine runs on a dedicated thread with its own tokio runtime.
    std::thread::Builder::new()
        .name("flux-fps-engine".into())
        .spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
            runtime.block_on(async move {
                match engine::runtime::run(Config::default(), command_rx).await {
                    Ok(telemetry) => {
                        let _ = ready_tx.send(telemetry);
                        // Keep this thread alive while the engine runs; the
                        // runtime loop exits when the UI sends Shutdown.
                        std::thread::sleep(std::time::Duration::from_secs(u64::MAX));
                    }
                    Err(e) => {
                        eprintln!("[flux-fps] engine failed: {e}");
                        std::process::exit(1);
                    }
                }
            });
        })
        .expect("engine thread");

    let telemetry = ready_rx.recv().expect("engine telemetry channel");
    // Blocks on the WinUI message loop until the window closes.
    App::run_component::<FluxFpsApp>(EngineHandles {
        commands,
        telemetry,
    })
    .expect("reactor app run");
}
