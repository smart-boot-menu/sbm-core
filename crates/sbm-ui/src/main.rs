//! SBM Core management shell.
//!
//! Privileged work goes through `sbm-cored`. This process never starts a shell.

mod theme;
mod typography;

use iced::widget::{Column, button, column, row, scrollable};
use iced::{Color, Element, Fill, Task, Theme};
use sbm_runtime::ipc::{
    ClientRequest, ClientResponse, HardwareInfo, LaunchTool, PowerAction, SystemSnapshot,
    ToolReport, transact,
};
use sbm_types::{DiskBus, OsFamily, PartitionProtectionClass};
use theme::Appearance;
use typography::{TextRole, UiScale, role_text};

pub fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .font(typography::ATKINSON_NEXT_REGULAR_BYTES)
        .font(typography::ATKINSON_NEXT_SEMIBOLD_BYTES)
        .font(typography::ATKINSON_MONO_REGULAR_BYTES)
        .default_font(typography::ATKINSON_NEXT_REGULAR)
        .window_size((1280.0, 800.0))
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Storage,
    OperatingSystems,
    Catalog,
    Recovery,
    Hardware,
    Network,
    Remote,
    Settings,
}

#[derive(Debug, Clone)]
enum Message {
    Snapshot(Result<ClientResponse, String>),
    Tools(Result<ClientResponse, String>),
    Remote(Result<ClientResponse, String>),
    Navigate(Page),
    Refresh,
    ProbeTools,
    Launch(LaunchTool),
    AskPower(PowerAction),
    ConfirmPower,
    CancelPower,
    RemoteEnable,
    RemoteDisable,
    ToggleTheme,
    ToggleHighContrast,
    ScaleUp,
    ScaleDown,
    ScaleReset,
}

struct App {
    page: Page,
    snapshot: Option<SystemSnapshot>,
    tools: Vec<ToolReport>,
    status: String,
    password: Option<String>,
    pending_power: Option<PowerAction>,
    announced: bool,
    appearance: Appearance,
    scale: UiScale,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        (
            Self {
                page: Page::Home,
                snapshot: None,
                tools: Vec::new(),
                status: "reading the system".to_string(),
                password: None,
                pending_power: None,
                announced: false,
                appearance: Appearance::default(),
                scale: UiScale::default(),
            },
            load_snapshot(),
        )
    }

    fn title(&self) -> String {
        if sbm_runtime::ipc::SIMULATED {
            "SBM Core - SIMULATED native preview (no real disks)".to_string()
        } else {
            "SBM Core".to_string()
        }
    }

    fn theme(&self) -> Theme {
        self.appearance.theme()
    }

    fn muted_color(&self) -> Color {
        self.appearance.muted_color()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Refresh => load_snapshot(),
            Message::ProbeTools => Task::perform(
                async { transact(&ClientRequest::ToolProbe) },
                Message::Tools,
            ),
            Message::Navigate(page) => {
                self.page = page;
                self.pending_power = None;
                if page == Page::Recovery && self.tools.is_empty() {
                    return Task::perform(
                        async { transact(&ClientRequest::ToolProbe) },
                        Message::Tools,
                    );
                }
                Task::none()
            }
            Message::Snapshot(Ok(ClientResponse::Snapshot { snapshot })) => {
                self.status = format!(
                    "{}  {}  {}",
                    snapshot.hostname, snapshot.sbm_version, snapshot.toram_mode
                );
                let announce = !self.announced;
                self.announced = true;
                self.snapshot = Some(*snapshot);
                if announce {
                    Task::perform(async { transact(&ClientRequest::UiReady) }, Message::Remote)
                } else {
                    Task::none()
                }
            }
            Message::Snapshot(Ok(other)) => {
                self.status = format!("unexpected reply: {other:?}");
                Task::none()
            }
            Message::Snapshot(Err(err)) => {
                self.status = err;
                if self.snapshot.is_none() {
                    Task::perform(
                        async {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            transact(&ClientRequest::Snapshot)
                        },
                        Message::Snapshot,
                    )
                } else {
                    Task::none()
                }
            }
            Message::Tools(Err(err)) | Message::Remote(Err(err)) => {
                self.status = err;
                Task::none()
            }
            Message::Tools(Ok(ClientResponse::Tools { tools })) => {
                self.tools = tools;
                Task::none()
            }
            Message::Remote(Ok(ClientResponse::Remote {
                enabled,
                detail,
                password,
            })) => {
                self.status = format!(
                    "remote {} — {detail}",
                    if enabled { "enabled" } else { "disabled" }
                );
                if password.is_some() {
                    self.password = password;
                }
                load_snapshot()
            }
            Message::Remote(Ok(ClientResponse::Ack { message })) => {
                self.status = message;
                Task::none()
            }
            Message::Remote(Ok(_)) => Task::none(),
            Message::Tools(Ok(_)) => Task::none(),
            Message::Launch(tool) => Task::perform(
                async move { transact(&ClientRequest::Launch { tool }) },
                Message::Remote,
            ),
            Message::AskPower(action) => {
                self.pending_power = Some(action);
                Task::none()
            }
            Message::CancelPower => {
                self.pending_power = None;
                Task::none()
            }
            Message::ConfirmPower => {
                let Some(action) = self.pending_power.clone() else {
                    return Task::none();
                };
                self.pending_power = None;
                self.status = "sending power request".to_string();
                Task::perform(
                    async move { transact(&ClientRequest::Power { action }) },
                    Message::Remote,
                )
            }
            Message::RemoteEnable => Task::perform(
                async { transact(&ClientRequest::RemoteEnable) },
                Message::Remote,
            ),
            Message::RemoteDisable => {
                self.password = None;
                Task::perform(
                    async { transact(&ClientRequest::RemoteDisable) },
                    Message::Remote,
                )
            }
            Message::ToggleTheme => {
                self.appearance.dark = !self.appearance.dark;
                Task::none()
            }
            Message::ToggleHighContrast => {
                self.appearance.high_contrast = !self.appearance.high_contrast;
                Task::none()
            }
            Message::ScaleUp => {
                self.scale = self.scale.step_up();
                Task::none()
            }
            Message::ScaleDown => {
                self.scale = self.scale.step_down();
                Task::none()
            }
            Message::ScaleReset => {
                self.scale = UiScale::default();
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let s = self.scale;
        let muted = self.muted_color();
        let nav = column![
            role_text("SBM Core", TextRole::Heading, s),
            nav_button("Home", Page::Home, self.page, s),
            nav_button("Storage", Page::Storage, self.page, s),
            nav_button("Operating systems", Page::OperatingSystems, self.page, s),
            nav_button("Install catalog", Page::Catalog, self.page, s),
            nav_button("Recovery", Page::Recovery, self.page, s),
            nav_button("Hardware", Page::Hardware, self.page, s),
            nav_button("Network", Page::Network, self.page, s),
            nav_button("Remote", Page::Remote, self.page, s),
            nav_button("Settings", Page::Settings, self.page, s),
            button(role_text("Refresh", TextRole::Body, s))
                .on_press(Message::Refresh)
                .style(button::text),
            role_text(&self.status, TextRole::Technical, s).color(muted),
        ]
        .spacing(8)
        .padding(16)
        .width(240);

        let body = scrollable(self.page_view().padding(20).spacing(10))
            .width(Fill)
            .height(Fill);

        row![nav, body].height(Fill).into()
    }

    fn page_view(&self) -> Column<'_, Message> {
        let s = self.scale;
        let muted = self.muted_color();
        let Some(snapshot) = &self.snapshot else {
            return column![
                role_text("Waiting for sbm-cored", TextRole::Display, s),
                role_text(
                    "The shell reads /run/sbm/cored.sock and does not run commands itself.",
                    TextRole::Body,
                    s,
                ),
                role_text(&self.status, TextRole::Technical, s).color(muted),
            ]
            .spacing(8);
        };
        match self.page {
            Page::Home => self.home(snapshot),
            Page::Storage => storage(snapshot, s, muted),
            Page::OperatingSystems => {
                let mut col = operating_systems(snapshot, s, muted);
                if self.pending_power.is_some() {
                    col = col.push(self.power_row());
                }
                col
            }
            Page::Catalog => column![
                role_text("Install catalog", TextRole::Heading, s),
                role_text(
                    "OS installation is not executed in SBM 0.1.",
                    TextRole::Body,
                    s
                ),
                role_text(
                    "This page is the placeholder for images published by sbm-server.",
                    TextRole::Caption,
                    s,
                )
                .color(muted),
            ]
            .spacing(8),
            Page::Recovery => self.recovery(snapshot),
            Page::Hardware => hardware(&snapshot.hardware, snapshot, s, muted),
            Page::Network => network(snapshot, s, muted),
            Page::Remote => self.remote(snapshot),
            Page::Settings => self.settings(snapshot),
        }
    }

    fn home(&self, snapshot: &SystemSnapshot) -> Column<'static, Message> {
        let s = self.scale;
        let muted = self.muted_color();
        let mut col = column![
            role_text(snapshot.hostname.clone(), TextRole::Display, s),
            row![
                role_text(snapshot.os_pretty.clone(), TextRole::Body, s),
                role_text("·", TextRole::Caption, s).color(muted),
                role_text(snapshot.kernel.clone(), TextRole::Technical, s).color(muted),
            ]
            .spacing(8),
            fact(
                "Version",
                &snapshot.sbm_version,
                TextRole::Technical,
                s,
                muted
            ),
            fact("Memory", memory_line(snapshot), TextRole::Body, s, muted),
            fact(
                "Root mode",
                &snapshot.toram_mode,
                TextRole::Technical,
                s,
                muted
            ),
            fact(
                "Backing disk",
                snapshot.backing_disk.as_deref().unwrap_or("none recorded"),
                TextRole::Technical,
                s,
                muted,
            ),
            fact(
                "Disks",
                disks_summary(snapshot),
                TextRole::Technical,
                s,
                muted
            ),
            fact("Handoff", &snapshot.handoff, TextRole::Technical, s, muted),
            fact(
                "Renderer",
                &snapshot.renderer,
                TextRole::Technical,
                s,
                muted
            ),
            fact("Remote", &snapshot.remote_detail, TextRole::Body, s, muted),
            row![
                button(role_text("Open terminal", TextRole::Body, s))
                    .on_press(Message::Launch(LaunchTool::Terminal))
                    .style(button::primary),
                button(role_text("Open GParted", TextRole::Body, s))
                    .on_press(Message::Launch(LaunchTool::Gparted))
                    .style(button::primary),
            ]
            .spacing(10),
        ]
        .spacing(8);
        if snapshot.backing_protected {
            col = col.push(role_text(
                "This system is running from its boot disk. Destroying that disk is refused.",
                TextRole::Caption,
                s,
            ));
        }
        col = col.push(self.power_row());
        col
    }

    fn recovery(&self, snapshot: &SystemSnapshot) -> Column<'static, Message> {
        let s = self.scale;
        let muted = self.muted_color();
        let mut col = column![
            role_text("Recovery tools", TextRole::Heading, s),
            role_text(
                "Inspection is available. SBM 0.1 does not apply partition changes from this shell.",
                TextRole::Caption,
                s,
            )
            .color(muted),
            button(role_text("Launch GParted", TextRole::Body, s))
                .on_press(Message::Launch(LaunchTool::Gparted))
                .style(button::primary),
            button(role_text("Open terminal", TextRole::Body, s))
                .on_press(Message::Launch(LaunchTool::Terminal))
                .style(button::text),
            button(role_text("Probe tool versions", TextRole::Body, s))
                .on_press(Message::ProbeTools)
                .style(button::text),
        ]
        .spacing(8);
        let tools = if self.tools.is_empty() {
            snapshot.tools.as_slice()
        } else {
            self.tools.as_slice()
        };
        for tool in tools {
            let mark = if tool.present { "present" } else { "missing" };
            col = col.push(role_text(
                format!(
                    "{mark:<7}  {:<12}  {}  {}",
                    tool.name, tool.path, tool.detail
                ),
                TextRole::Technical,
                s,
            ));
        }
        col
    }

    fn remote(&self, snapshot: &SystemSnapshot) -> Column<'static, Message> {
        let s = self.scale;
        let muted = self.muted_color();
        let mut col = column![
            role_text("Remote access", TextRole::Heading, s),
            role_text(snapshot.remote_detail.clone(), TextRole::Body, s),
            role_text(
                "wayvnc stays off until an operator enables it. It binds to 127.0.0.1 and requires a password.",
                TextRole::Caption,
                s,
            )
            .color(muted),
            row![
                button(role_text("Enable", TextRole::Body, s))
                    .on_press(Message::RemoteEnable)
                    .style(button::primary),
                button(role_text("Disable", TextRole::Body, s))
                    .on_press(Message::RemoteDisable)
                    .style(button::danger),
            ]
            .spacing(10),
        ]
        .spacing(8);
        if let Some(password) = &self.password {
            col = col.push(role_text(
                format!("Password for user sbm: {password}"),
                TextRole::Technical,
                s,
            ));
            col = col.push(
                role_text(
                    "This password is shown once in this session.",
                    TextRole::Caption,
                    s,
                )
                .color(muted),
            );
        }
        col
    }

    fn settings(&self, snapshot: &SystemSnapshot) -> Column<'static, Message> {
        let s = self.scale;
        let muted = self.muted_color();
        let mut col = column![
            role_text("Settings", TextRole::Heading, s),
            fact("Hostname", &snapshot.hostname, TextRole::Technical, s, muted),
            fact("Architecture", &snapshot.arch, TextRole::Technical, s, muted),
            fact(
                "Uptime",
                format!("{} s", snapshot.uptime_secs),
                TextRole::Technical,
                s,
                muted,
            ),
            fact(
                "Handoff verified",
                if snapshot.handoff_verified { "yes" } else { "no" },
                TextRole::Body,
                s,
                muted,
            ),
            fact("Renderer", &snapshot.renderer, TextRole::Technical, s, muted),
            role_text("Appearance & accessibility", TextRole::Heading, s),
            fact("Theme", self.appearance.label(), TextRole::Body, s, muted),
            fact(
                "Interface scale",
                format!("{}%", self.scale.percent()),
                TextRole::Technical,
                s,
                muted,
            ),
            fact(
                "Typography",
                "Atkinson Hyperlegible Next + Mono",
                TextRole::Body,
                s,
                muted,
            ),
            row![
                button(role_text(
                    if self.appearance.dark { "Switch to light" } else { "Switch to dark" },
                    TextRole::Body,
                    s,
                ))
                .on_press(Message::ToggleTheme)
                .style(button::primary),
                button(role_text(
                    if self.appearance.high_contrast {
                        "Standard contrast"
                    } else {
                        "High contrast"
                    },
                    TextRole::Body,
                    s,
                ))
                .on_press(Message::ToggleHighContrast)
                .style(button::text),
                button(role_text("Smaller", TextRole::Body, s))
                    .on_press(Message::ScaleDown)
                    .style(button::text),
                button(role_text("100%", TextRole::Technical, s))
                    .on_press(Message::ScaleReset)
                    .style(button::text),
                button(role_text("Larger", TextRole::Body, s))
                    .on_press(Message::ScaleUp)
                    .style(button::text),
            ]
            .spacing(10),
            role_text(
                "Software rendering: boot with sbm.renderer=software or write \"software\" to \\EFI\\sbm\\state\\renderer.",
                TextRole::Caption,
                s,
            )
            .color(muted),
        ]
        .spacing(8);
        col = col.push(self.power_row());
        col
    }

    fn power_row(&self) -> Column<'static, Message> {
        let s = self.scale;
        let mut col = column![
            row![
                button(role_text("Reboot", TextRole::Body, s))
                    .on_press(Message::AskPower(PowerAction::Reboot))
                    .style(button::danger),
                button(role_text("Shut down", TextRole::Body, s))
                    .on_press(Message::AskPower(PowerAction::Shutdown))
                    .style(button::danger),
                button(role_text("Return to boot manager", TextRole::Body, s))
                    .on_press(Message::AskPower(PowerAction::BootManager))
                    .style(button::text),
            ]
            .spacing(10)
        ]
        .spacing(8);
        if let Some(action) = &self.pending_power {
            col = col.push(role_text(
                format!("Confirm {}?", power_label(action)),
                TextRole::Heading,
                s,
            ));
            col = col.push(
                row![
                    button(role_text("Confirm", TextRole::Body, s))
                        .on_press(Message::ConfirmPower)
                        .style(button::danger),
                    button(role_text("Cancel", TextRole::Body, s))
                        .on_press(Message::CancelPower)
                        .style(button::text),
                ]
                .spacing(10),
            );
        }
        col
    }
}

fn disks_summary(snapshot: &SystemSnapshot) -> String {
    if snapshot.disks.is_empty() {
        return "none reported".to_string();
    }
    let parts: Vec<String> = snapshot
        .disks
        .iter()
        .map(|disk| format!("{} ({})", disk.device_path, bytes(disk.identity.size_bytes)))
        .collect();
    format!("{} detected: {}", snapshot.disks.len(), parts.join(", "))
}

fn storage(snapshot: &SystemSnapshot, s: UiScale, muted: Color) -> Column<'_, Message> {
    let mut col = column![
        role_text("Storage", TextRole::Heading, s),
        role_text(
            "Free space is shown only for filesystems that are already mounted.",
            TextRole::Caption,
            s,
        )
        .color(muted),
    ]
    .spacing(8);
    if snapshot.disks.is_empty() {
        col = col.push(role_text("No disks reported.", TextRole::Body, s));
    }
    for disk in &snapshot.disks {
        col = col.push(
            row![
                role_text(disk.identity.model.clone(), TextRole::Heading, s),
                role_text(
                    format!(
                        "{}  ·  {}  ·  {}",
                        bytes(disk.identity.size_bytes),
                        bus_label(disk.identity.bus),
                        disk.health
                    ),
                    TextRole::Body,
                    s,
                )
                .color(muted),
            ]
            .spacing(10),
        );
        col = col.push(
            role_text(
                format!("{}  id {}", disk.device_path, disk.identity.stable_id),
                TextRole::Technical,
                s,
            )
            .color(muted),
        );
        for part in &disk.partitions {
            let avail = part
                .fs_avail_bytes
                .map(bytes)
                .unwrap_or_else(|| "n/a".to_string());
            col = col.push(role_text(
                format!(
                    "    {}  {}  {}  {}  mounted {}  free {}  {}{}",
                    part.device_path,
                    part.fstype.as_deref().unwrap_or("unknown"),
                    bytes(part.size_bytes),
                    protection_label(part.protection),
                    part.mountpoint.as_deref().unwrap_or("no"),
                    avail,
                    part.partuuid.as_deref().unwrap_or(""),
                    if part.encrypted { "  encrypted" } else { "" }
                ),
                TextRole::Technical,
                s,
            ));
        }
    }
    col
}

fn operating_systems(snapshot: &SystemSnapshot, s: UiScale, muted: Color) -> Column<'_, Message> {
    let mut col = column![
        role_text("Installed operating systems", TextRole::Heading, s),
        role_text(
            "Identities come from partition types and superblocks. Nothing is mounted to discover them.",
            TextRole::Caption,
            s,
        )
        .color(muted),
    ]
    .spacing(8);
    if snapshot.installed_os.is_empty() {
        col = col.push(role_text(
            "No installed system was inferred.",
            TextRole::Body,
            s,
        ));
    }
    for os in &snapshot.installed_os {
        col = col.push(
            row![
                role_text(os.display_name.clone(), TextRole::Heading, s),
                role_text(os_family_label(os.os_family), TextRole::Caption, s).color(muted),
            ]
            .spacing(10),
        );
        col = col.push(role_text(os.evidence.clone(), TextRole::Technical, s).color(muted));
    }
    col = col.push(role_text("Boot targets", TextRole::Heading, s));
    if snapshot.boot_targets.is_empty() {
        col = col.push(
            role_text(
                "No boot targets available from the EFI picker.",
                TextRole::Caption,
                s,
            )
            .color(muted),
        );
    }
    for target in &snapshot.boot_targets {
        col = col.push(role_text(target.display_name.clone(), TextRole::Heading, s));
        if let Some(path) = target.efi_path.as_ref() {
            col = col.push(role_text(path.clone(), TextRole::Technical, s).color(muted));
        }
        col = col.push(
            button(role_text(
                "Request this target on the next boot",
                TextRole::Body,
                s,
            ))
            .on_press(Message::AskPower(PowerAction::BootTarget {
                target_id: target.id.as_str().to_owned(),
            }))
            .style(button::text),
        );
    }
    col
}

fn hardware(
    info: &HardwareInfo,
    snapshot: &SystemSnapshot,
    s: UiScale,
    muted: Color,
) -> Column<'static, Message> {
    let gpus = if info.gpus.is_empty() {
        "none reported".to_string()
    } else {
        info.gpus.join(", ")
    };
    let headline = format!(
        "{} · {} cores · {}",
        info.arch,
        info.cores,
        kib(snapshot.memory.total_kib)
    );
    column![
        role_text("Hardware", TextRole::Heading, s),
        role_text(headline, TextRole::Display, s),
        fact("Vendor", &info.vendor, TextRole::Body, s, muted),
        fact("Product", &info.product, TextRole::Body, s, muted),
        fact("CPU", &info.cpu_model, TextRole::Body, s, muted),
        fact(
            "Cores",
            info.cores.to_string(),
            TextRole::Technical,
            s,
            muted
        ),
        fact("Architecture", &info.arch, TextRole::Technical, s, muted),
        fact("GPUs", &gpus, TextRole::Body, s, muted),
        fact(
            "Memory",
            memory_line(snapshot),
            TextRole::Technical,
            s,
            muted
        ),
        fact("Kernel", &snapshot.kernel, TextRole::Technical, s, muted),
    ]
    .spacing(8)
}

fn network(snapshot: &SystemSnapshot, s: UiScale, muted: Color) -> Column<'_, Message> {
    let mut col = column![
        role_text("Network", TextRole::Heading, s),
        role_text(
            "Ethernet and Wi-Fi use DHCP when a link is up. No network is required to boot.",
            TextRole::Caption,
            s,
        )
        .color(muted),
    ]
    .spacing(8);
    if snapshot.nics.is_empty() {
        col = col.push(role_text("No interfaces reported.", TextRole::Body, s));
    }
    for nic in &snapshot.nics {
        let kind = if nic.wireless { "wifi" } else { "ethernet" };
        let addrs = if nic.addresses.is_empty() {
            "no address".to_string()
        } else {
            nic.addresses.join(", ")
        };
        col = col.push(role_text(
            format!(
                "{}  {}  {}  {}  {}",
                nic.name,
                kind,
                nic.state,
                nic.mac.as_deref().unwrap_or(""),
                addrs
            ),
            TextRole::Technical,
            s,
        ));
    }
    col
}

fn nav_button<'a>(
    label: &'a str,
    page: Page,
    current: Page,
    scale: UiScale,
) -> Element<'a, Message> {
    let control = button(role_text(label, TextRole::Body, scale))
        .on_press(Message::Navigate(page))
        .width(Fill);
    if page == current {
        control.style(button::primary).into()
    } else {
        control.style(button::text).into()
    }
}

fn load_snapshot() -> Task<Message> {
    Task::perform(
        async { transact(&ClientRequest::Snapshot) },
        Message::Snapshot,
    )
}

fn fact(
    label: &'static str,
    value: impl AsRef<str>,
    value_role: TextRole,
    scale: UiScale,
    muted: Color,
) -> Element<'static, Message> {
    row![
        role_text(label, TextRole::Caption, scale)
            .color(muted)
            .width(160),
        role_text(value.as_ref().to_string(), value_role, scale),
    ]
    .spacing(8)
    .into()
}

fn memory_line(snapshot: &SystemSnapshot) -> String {
    format!(
        "{} total, {} available",
        kib(snapshot.memory.total_kib),
        kib(snapshot.memory.available_kib)
    )
}

fn kib(value: u64) -> String {
    bytes(value.saturating_mul(1024))
}

fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn bus_label(bus: DiskBus) -> &'static str {
    match bus {
        DiskBus::Nvme => "NVMe",
        DiskBus::Sata => "SATA",
        DiskBus::Usb => "USB",
        DiskBus::Virtio => "virtio",
        DiskBus::Scsi => "SCSI",
        DiskBus::Mmc => "MMC",
        DiskBus::Unknown => "unknown bus",
    }
}

fn protection_label(class: PartitionProtectionClass) -> &'static str {
    match class {
        PartitionProtectionClass::Normal => "normal",
        PartitionProtectionClass::ProtectedEsp => "ESP",
        PartitionProtectionClass::ProtectedAppleSystem => "Apple system",
        PartitionProtectionClass::ProtectedEncrypted => "encrypted",
        PartitionProtectionClass::ProtectedSbmRecovery => "SBM recovery",
    }
}

fn os_family_label(family: OsFamily) -> &'static str {
    match family {
        OsFamily::Linux => "Linux",
        OsFamily::Windows => "Windows",
        OsFamily::MacOs => "macOS",
        OsFamily::Bsd => "BSD",
        OsFamily::Recovery => "recovery",
        OsFamily::FirmwareUtility => "firmware",
        OsFamily::Other => "other",
    }
}

fn power_label(action: &PowerAction) -> String {
    match action {
        PowerAction::Reboot => "reboot".to_string(),
        PowerAction::Shutdown => "shut down".to_string(),
        PowerAction::BootManager => "return to the boot manager".to_string(),
        PowerAction::BootTarget { target_id } => format!("boot {target_id}"),
    }
}
