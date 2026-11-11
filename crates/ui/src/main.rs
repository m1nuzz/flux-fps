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
//!
//! The settings panel edits a local `Config` draft: every change is sent to
//! the engine immediately (so it takes effect without a restart) and an
//! explicit Save persists it to `%APPDATA%/flux-fps/config.toml`.
//!
//! The same executable also hosts the elevated helper (`--helper`) and the
//! uninstall restore hook (`--restore`); both are dispatched before any UI
//! or engine thread is created.

use common::{
    normalize_whitelist_entry, Command, Config, HelperReply, HelperRequest, TelemetrySnapshot,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use windows_reactor::{
    App, Button, ChildrenControl, Component, ComponentContext, ContentControl, KeyedView,
    Orientation, StackPanel, TextBlock, TextBox, ToggleSwitch, View,
};

mod chart;
mod tray;

/// Engine-side handles handed to the root component as its Input.
///
/// `run_component` requires `Input: Clone + PartialEq`; the channels are
/// shared behind `Arc`s and compare equal only for the same instance.
#[derive(Clone)]
pub struct EngineHandles {
    pub commands: Arc<mpsc::Sender<Command>>,
    pub telemetry: Arc<watch::Receiver<TelemetrySnapshot>>,
    pub config_path: Arc<PathBuf>,
}

impl PartialEq for EngineHandles {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.commands, &other.commands)
            && Arc::ptr_eq(&self.telemetry, &other.telemetry)
            && Arc::ptr_eq(&self.config_path, &other.config_path)
    }
}

/// Messages delivered to the root component's update loop.
#[derive(Clone)]
pub enum Message {
    /// Latest telemetry snapshot from the engine (30 Hz poll cycle).
    Telemetry(TelemetrySnapshot),
    /// User asked for an immediate standby purge.
    PurgeStandbyNow,
    /// User toggled Game Mode on.
    GameModeOn,
    /// User toggled Game Mode off.
    GameModeOff,
    /// Text typed into the whitelist entry box.
    WhitelistInput(String),
    /// Add the typed process name to the protected games list.
    AddWhitelist,
    /// Remove one process name from the protected games list.
    RemoveWhitelist(String),
    /// Adjust the commit-charge purge threshold, percentage points.
    AdjustPurgeCommit(i8),
    /// Adjust the available-memory purge threshold, MB.
    AdjustPurgeAvailable(i32),
    /// Adjust the purge cooldown, seconds.
    AdjustPurgeCooldown(i64),
    /// Enable/disable the adaptive FPS limit advisor.
    SetPacerEnabled(bool),
    /// Adjust the pacer safety factor (aggressiveness).
    AdjustPacerSafety(f32),
    /// Enable/disable logon auto-start (applied by the helper/installer).
    SetAutoStart(bool),
    /// Persist the current settings draft to disk.
    SaveSettings,
    /// Probe the elevated helper pipe on a background thread.
    CheckHelper,
    /// Result of the helper probe, displayed in the settings panel.
    HelperStatus(String),
}

/// Local editable copy of the configuration plus transient UI state.
struct SettingsDraft {
    config: Config,
    whitelist_input: String,
    status: String,
}

pub struct FluxFpsApp {
    commands: Arc<mpsc::Sender<Command>>,
    /// Engine-side watch receiver used to re-arm each telemetry poll.
    telemetry: Arc<watch::Receiver<TelemetrySnapshot>>,
    config_path: Arc<PathBuf>,
    latest: TelemetrySnapshot,
    settings: SettingsDraft,
}

impl FluxFpsApp {
    fn send(&self, command: Command) {
        // Bounded channel: the UI thread must use try_send only, never block.
        let _ = self.commands.try_send(command);
    }

    fn purge_thresholds_command(&self) -> Command {
        let config = &self.settings.config;
        Command::SetPurgeThresholds {
            commit_percent: config.purge_commit_threshold_percent,
            available_mb: config.purge_available_threshold_mb,
            cooldown_secs: config.purge_cooldown_secs,
        }
    }

    /// Settings section. Nested panels keep every `children()` tuple below
    /// the reactor's static 16-element limit.
    fn settings_view(&self, context: &mut windows_reactor::ViewContext<Self>) -> View {
        let config = &self.settings.config;

        let rows: Vec<KeyedView> = config
            .game_whitelist
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let row = StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(6.0)
                    .children((
                        TextBlock::new().text(name.clone()),
                        Button::new()
                            .on_click(
                                context
                                    .sender()
                                    .message(Message::RemoveWhitelist(name.clone())),
                            )
                            .content("Remove"),
                    ));
                KeyedView::new(index as u32, row)
            })
            .collect();
        let whitelist_rows = StackPanel::new().spacing(2.0).keyed_children(rows);

        let whitelist_section = StackPanel::new().spacing(4.0).children((
            TextBlock::new().text("Protected games (never isolated):"),
            whitelist_rows,
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(6.0)
                .children((
                    TextBox::new()
                        .text(self.settings.whitelist_input.clone())
                        .placeholder_text("game.exe")
                        .on_text_changed(
                            context
                                .sender()
                                .callback::<String, _>(Message::WhitelistInput),
                        ),
                    Button::new()
                        .on_click(context.sender().message(Message::AddWhitelist))
                        .content("Add"),
                )),
        ));

        let purge_section = StackPanel::new().spacing(4.0).children((
            TextBlock::new().text(format!(
                "Purge commit threshold: {}%",
                config.purge_commit_threshold_percent
            )),
            adjustment_row(
                context,
                Message::AdjustPurgeCommit(-5),
                Message::AdjustPurgeCommit(5),
                "-5%",
                "+5%",
            ),
            TextBlock::new().text(format!(
                "Purge available-memory threshold: {} MB",
                config.purge_available_threshold_mb
            )),
            adjustment_row(
                context,
                Message::AdjustPurgeAvailable(-256),
                Message::AdjustPurgeAvailable(256),
                "-256 MB",
                "+256 MB",
            ),
            TextBlock::new().text(format!("Purge cooldown: {} s", config.purge_cooldown_secs)),
            adjustment_row(
                context,
                Message::AdjustPurgeCooldown(-10),
                Message::AdjustPurgeCooldown(10),
                "-10 s",
                "+10 s",
            ),
        ));

        let pacer_section = StackPanel::new().spacing(4.0).children((
            TextBlock::new().text("Adaptive FPS limit advisor:"),
            ToggleSwitch::new().is_on(config.pacer_enabled).on_toggled(
                context
                    .sender()
                    .callback::<bool, _>(Message::SetPacerEnabled),
            ),
            TextBlock::new().text(format!(
                "Pacer safety factor: {:.2} (higher = more conservative)",
                config.pacer_safety_factor
            )),
            adjustment_row(
                context,
                Message::AdjustPacerSafety(-0.05),
                Message::AdjustPacerSafety(0.05),
                "More aggressive",
                "Safer",
            ),
        ));

        let startup_section = StackPanel::new().spacing(4.0).children((
            TextBlock::new().text("Start with Windows (applied by installer/helper):"),
            ToggleSwitch::new()
                .is_on(config.auto_start)
                .on_toggled(context.sender().callback::<bool, _>(Message::SetAutoStart)),
            Button::new()
                .on_click(context.sender().message(Message::SaveSettings))
                .content("Save settings"),
            Button::new()
                .on_click(context.sender().message(Message::CheckHelper))
                .content("Check helper connection"),
            TextBlock::new().text(self.settings.status.clone()),
        ));

        StackPanel::new().spacing(8.0).children((
            TextBlock::new().text("Settings"),
            whitelist_section,
            purge_section,
            pacer_section,
            startup_section,
        ))
    }
}

/// Two-button +/- row used by every numeric setting.
fn adjustment_row(
    context: &windows_reactor::ViewContext<FluxFpsApp>,
    decrease: Message,
    increase: Message,
    decrease_label: &'static str,
    increase_label: &'static str,
) -> View {
    StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(4.0)
        .children((
            Button::new()
                .on_click(context.sender().message(decrease))
                .content(decrease_label),
            Button::new()
                .on_click(context.sender().message(increase))
                .content(increase_label),
        ))
}

impl Component for FluxFpsApp {
    type Input = EngineHandles;
    type Message = Message;

    fn create(input: &EngineHandles, context: &ComponentContext<Self>) -> Self {
        spawn_telemetry_poll(input.telemetry.clone(), context);
        Self {
            commands: input.commands.clone(),
            telemetry: input.telemetry.clone(),
            config_path: input.config_path.clone(),
            latest: TelemetrySnapshot::default(),
            settings: SettingsDraft {
                config: Config::load_or_default(input.config_path.as_path()),
                whitelist_input: String::new(),
                status: "Changes apply immediately; Save writes config.toml.".into(),
            },
        }
    }

    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        match message {
            Message::Telemetry(snapshot) => {
                self.latest = snapshot;
                // Re-arm the poll loop (self-perpetuating 30 Hz cycle).
                spawn_telemetry_poll(self.telemetry.clone(), context);
            }
            Message::PurgeStandbyNow => self.send(Command::PurgeStandbyNow),
            Message::GameModeOn => self.send(Command::SetGameModeEnabled(true)),
            Message::GameModeOff => self.send(Command::SetGameModeEnabled(false)),
            Message::WhitelistInput(text) => self.settings.whitelist_input = text,
            Message::AddWhitelist => {
                let entry = normalize_whitelist_entry(&self.settings.whitelist_input);
                match entry {
                    Some(name) => {
                        if self.settings.config.game_whitelist.contains(&name) {
                            self.settings.status = format!("{name} is already protected.");
                        } else {
                            self.settings.config.game_whitelist.push(name.clone());
                            self.send(Command::SetGameWhitelist(
                                self.settings.config.game_whitelist.clone(),
                            ));
                            self.settings.status = format!("Protected {name}.");
                        }
                        self.settings.whitelist_input.clear();
                    }
                    None => {
                        self.settings.status =
                            "Enter a plain .exe name (for example cs2.exe).".into();
                    }
                }
            }
            Message::RemoveWhitelist(name) => {
                self.settings
                    .config
                    .game_whitelist
                    .retain(|entry| entry != &name);
                self.send(Command::SetGameWhitelist(
                    self.settings.config.game_whitelist.clone(),
                ));
                self.settings.status = format!("Removed {name}.");
            }
            Message::AdjustPurgeCommit(delta) => {
                let config = &mut self.settings.config;
                let next = (i16::from(config.purge_commit_threshold_percent) + i16::from(delta))
                    .clamp(50, 99) as u8;
                config.purge_commit_threshold_percent = next;
                self.send(self.purge_thresholds_command());
            }
            Message::AdjustPurgeAvailable(delta) => {
                let config = &mut self.settings.config;
                let next = (i64::from(config.purge_available_threshold_mb) + i64::from(delta))
                    .clamp(512, 16_384) as u32;
                config.purge_available_threshold_mb = next;
                self.send(self.purge_thresholds_command());
            }
            Message::AdjustPurgeCooldown(delta) => {
                let config = &mut self.settings.config;
                let next = (config.purge_cooldown_secs.min(i64::MAX as u64) as i64 + delta)
                    .clamp(10, 600) as u64;
                config.purge_cooldown_secs = next;
                self.send(self.purge_thresholds_command());
            }
            Message::SetPacerEnabled(on) => {
                self.settings.config.pacer_enabled = on;
                self.send(Command::SetPacerEnabled(on));
            }
            Message::AdjustPacerSafety(delta) => {
                let factor = {
                    let config = &mut self.settings.config;
                    let quantized = ((config.pacer_safety_factor + delta) * 20.0).round() / 20.0;
                    config.pacer_safety_factor = quantized.clamp(1.0, 2.0);
                    config.pacer_safety_factor
                };
                self.send(Command::SetPacerSafetyFactor(factor));
            }
            Message::SetAutoStart(on) => {
                self.settings.config.auto_start = on;
                self.settings.status =
                    "Auto-start is applied by the helper at the next logon.".into();
            }
            Message::SaveSettings => {
                self.settings.status = match self.settings.config.save(self.config_path.as_path()) {
                    Ok(()) => "Settings saved to config.toml.".into(),
                    Err(e) => format!("Save failed: {e}"),
                };
            }
            Message::CheckHelper => {
                // The blocking Win32 pipe calls must never run on the UI
                // thread; the reactor thread pool returns a normal Message.
                context.spawn_background(|_| {
                    let status = match engine::pipe::PipeClient::connect() {
                        Ok(client) => match client.call(&HelperRequest::Ping) {
                            Ok(HelperReply::Pong { version, elevated }) => format!(
                                "Helper connected (protocol v{version}, elevated: {elevated})."
                            ),
                            Ok(HelperReply::Ok) | Ok(HelperReply::Error(_)) => {
                                "Helper returned an unexpected reply.".into()
                            }
                            Err(e) => format!("Helper request failed: {e}"),
                        },
                        Err(e) => {
                            format!("Helper unavailable ({e}); user-mode fallback active.")
                        }
                    };
                    Message::HelperStatus(status)
                });
            }
            Message::HelperStatus(status) => self.settings.status = status,
        }
    }

    fn view(&self, _input: &Self::Input, context: &mut windows_reactor::ViewContext<Self>) -> View {
        let dpc_peak = self
            .latest
            .dpc_max_us
            .iter()
            .copied()
            .fold(0.0f32, f32::max);
        StackPanel::new().spacing(8.0).children((
            TextBlock::new().text(format!("flux-fps v{}", common::VERSION)),
            TextBlock::new().text(format!(
                "Memory: commit {:.0}%, available {} MB",
                self.latest.commit_percent, self.latest.available_mb
            )),
            TextBlock::new().text(format!(
                "Game Mode: {} background processes managed",
                self.latest.managed_pids.len()
            )),
            TextBlock::new().text("CPU cores (blue = P, green = E):"),
            chart::cpu_map(&self.latest.cpu_cores),
            TextBlock::new().text(format!("Peak DPC: {dpc_peak:.1} us")),
            chart::dpc_chart(&self.latest),
            TextBlock::new().text(chart::frametime_text(&self.latest.frametimes)),
            TextBlock::new().text(chart::pacer_text(&self.latest.pacer)),
            TextBlock::new().text(format!("FPS cap: {:?}", self.latest.fps_cap)),
            Button::new()
                .on_click(context.sender().message(Message::PurgeStandbyNow))
                .content("Purge standby list now"),
            Button::new()
                .on_click(context.sender().message(if self.latest.game_mode_active {
                    Message::GameModeOff
                } else {
                    Message::GameModeOn
                }))
                .content(if self.latest.game_mode_active {
                    "Disable Game Mode"
                } else {
                    "Enable Game Mode"
                }),
            self.settings_view(context),
        ))
    }
}

/// Arms one telemetry poll: sleeps ~33 ms on the Windows thread pool, clones
/// the latest engine snapshot and delivers it as a Message. `update()` re-arms.
fn spawn_telemetry_poll(
    telemetry: Arc<watch::Receiver<TelemetrySnapshot>>,
    context: &ComponentContext<FluxFpsApp>,
) {
    context.spawn_background(move |_cancellation| {
        std::thread::sleep(std::time::Duration::from_millis(33));
        Message::Telemetry(telemetry.borrow().clone())
    });
}

fn main() {
    // Non-UI modes are dispatched before any window, tray or engine thread
    // exists. `--helper` is launched elevated by the Task Scheduler task;
    // `--restore` is called by the MSI uninstall custom action.
    match std::env::args().nth(1).as_deref() {
        Some("--helper") => {
            if let Err(e) = engine::helper::run() {
                eprintln!("[flux-fps] helper failed: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some("--restore") => {
            if let Err(e) = engine::helper::restore_all() {
                eprintln!("[flux-fps] restore failed: {e}");
                std::process::exit(1);
            }
            println!("[flux-fps] isolation state restored.");
            return;
        }
        Some("--version") => {
            println!("flux-fps v{}", common::VERSION);
            return;
        }
        Some(other) => eprintln!("[flux-fps] ignoring unknown argument: {other}"),
        None => {}
    }

    println!("flux-fps v{} starting", common::VERSION);

    let config_path = Config::default_path().unwrap_or_else(|| {
        eprintln!("[flux-fps] APPDATA unavailable; using local config.toml");
        "config.toml".into()
    });
    let config = Config::load_or_default(&config_path);

    let (commands, command_rx) = mpsc::channel::<Command>(16);
    // Tray icon on its own message-only window thread; shares the command
    // channel (try_send only, never blocks).
    let tray_handle = tray::spawn(commands.clone());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<watch::Receiver<TelemetrySnapshot>>();

    // The Core Engine runs on a dedicated thread with its own tokio runtime.
    std::thread::Builder::new()
        .name("flux-fps-engine".into())
        .spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
            runtime.block_on(async move {
                match engine::runtime::run(config, command_rx).await {
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
    let result = App::run_component::<FluxFpsApp>(EngineHandles {
        commands: Arc::new(commands),
        telemetry: Arc::new(telemetry),
        config_path: Arc::new(config_path),
    });

    // Graceful teardown: remove the tray icon, then let the engine thread
    // wind down with the process.
    tray_handle.shutdown();
    result.expect("reactor app run");
}
