//! SBM Core management shell.
//!
//! Privileged work goes through `sbm-cored`. This process never starts a shell.

mod theme;

use iced::widget::{Column, button, column, row, scrollable, text};
use iced::{Element, Fill, Task, Theme};
use sbm_runtime::ipc::{
    ClientRequest, ClientResponse, HardwareInfo, LaunchTool, PowerAction, SystemSnapshot,
    ToolReport, transact,
};
use sbm_types::{DiskBus, OsFamily, PartitionProtectionClass};

pub fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
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
}

struct App {
    page: Page,
    snapshot: Option<SystemSnapshot>,
    tools: Vec<ToolReport>,
    status: String,
    password: Option<String>,
    pending_power: Option<PowerAction>,
    announced: bool,
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
            },
            load_snapshot(),
        )
    }

    fn title(&self) -> String {
        "SBM Core".to_string()
    }

    fn theme(&self) -> Theme {
        theme::theme()
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
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let nav = column![
            text("SBM Core").size(22),
            nav_button("Home", Page::Home, self.page),
            nav_button("Storage", Page::Storage, self.page),
            nav_button("Operating systems", Page::OperatingSystems, self.page),
            nav_button("Install catalog", Page::Catalog, self.page),
            nav_button("Recovery", Page::Recovery, self.page),
            nav_button("Hardware", Page::Hardware, self.page),
            nav_button("Network", Page::Network, self.page),
            nav_button("Remote", Page::Remote, self.page),
            nav_button("Settings", Page::Settings, self.page),
            button("Refresh")
                .on_press(Message::Refresh)
                .style(button::text),
            text(&self.status).size(14).style(muted),
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
        let Some(snapshot) = &self.snapshot else {
            return column![
                text("Waiting for sbm-cored").size(28),
                text("The shell reads /run/sbm/cored.sock and does not run commands itself."),
                text(&self.status).style(muted),
            ]
            .spacing(8);
        };
        match self.page {
            Page::Home => self.home(snapshot),
            Page::Storage => storage(snapshot),
            Page::OperatingSystems => {
                let mut col = operating_systems(snapshot);
                if self.pending_power.is_some() {
                    col = col.push(self.power_row());
                }
                col
            }
            Page::Catalog => column![
                heading("Install catalog"),
                text("OS installation is not executed in SBM 0.1."),
                text("This page is the placeholder for images published by sbm-server.")
                    .style(muted),
            ]
            .spacing(8),
            Page::Recovery => self.recovery(snapshot),
            Page::Hardware => hardware(&snapshot.hardware, snapshot),
            Page::Network => network(snapshot),
            Page::Remote => self.remote(snapshot),
            Page::Settings => self.settings(snapshot),
        }
    }

    fn home(&self, snapshot: &SystemSnapshot) -> Column<'static, Message> {
        let mut col = column![
            heading("Home"),
            text(format!("{}  ·  {}", snapshot.os_pretty, snapshot.kernel)),
            fact("Version", &snapshot.sbm_version),
            fact("Memory", memory_line(snapshot)),
            fact("Root mode", &snapshot.toram_mode),
            fact(
                "Backing disk",
                snapshot.backing_disk.as_deref().unwrap_or("none recorded"),
            ),
            fact("Disks", disks_summary(snapshot)),
            fact("Handoff", &snapshot.handoff),
            fact("Renderer", &snapshot.renderer),
            fact("Remote", &snapshot.remote_detail),
            row![
                button("Open terminal")
                    .on_press(Message::Launch(LaunchTool::Terminal))
                    .style(button::primary),
                button("Open GParted")
                    .on_press(Message::Launch(LaunchTool::Gparted))
                    .style(button::primary),
            ]
            .spacing(10),
        ]
        .spacing(8);
        if snapshot.backing_protected {
            col = col.push(text(
                "This system is running from its boot disk. Destroying that disk is refused.",
            ));
        }
        col = col.push(self.power_row());
        col
    }

    fn recovery(&self, snapshot: &SystemSnapshot) -> Column<'static, Message> {
        let mut col = column![
            heading("Recovery tools"),
            text(
                "Inspection is available. SBM 0.1 does not apply partition changes from this shell."
            )
            .style(muted),
            button("Launch GParted")
                .on_press(Message::Launch(LaunchTool::Gparted))
                .style(button::primary),
            button("Open terminal")
                .on_press(Message::Launch(LaunchTool::Terminal))
                .style(button::text),
            button("Probe tool versions")
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
            col = col.push(text(format!(
                "{mark}  {}  {}  {}",
                tool.name, tool.path, tool.detail
            )));
        }
        col
    }

    fn remote(&self, snapshot: &SystemSnapshot) -> Column<'static, Message> {
        let mut col = column![
            heading("Remote access"),
            text(snapshot.remote_detail.clone()),
            text("wayvnc stays off until an operator enables it. It binds to 127.0.0.1 and requires a password.")
                .style(muted),
            row![
                button("Enable").on_press(Message::RemoteEnable).style(button::primary),
                button("Disable").on_press(Message::RemoteDisable).style(button::danger),
            ]
            .spacing(10),
        ]
        .spacing(8);
        if let Some(password) = &self.password {
            col = col.push(text(format!("Password for user sbm: {password}")));
            col = col.push(text("This password is shown once in this session.").style(muted));
        }
        col
    }

    fn settings(&self, snapshot: &SystemSnapshot) -> Column<'static, Message> {
        let mut col = column![
            heading("Settings"),
            fact("Hostname", &snapshot.hostname),
            fact("Architecture", &snapshot.arch),
            fact("Uptime", format!("{} s", snapshot.uptime_secs)),
            fact(
                "Handoff verified",
                if snapshot.handoff_verified { "yes" } else { "no" }
            ),
            fact("Renderer", &snapshot.renderer),
            text("Software rendering: boot with sbm.renderer=software or write \"software\" to \\EFI\\sbm\\state\\renderer.")
                .style(muted),
        ]
        .spacing(8);
        col = col.push(self.power_row());
        col
    }

    fn power_row(&self) -> Column<'static, Message> {
        let mut col = column![
            row![
                button("Reboot")
                    .on_press(Message::AskPower(PowerAction::Reboot))
                    .style(button::danger),
                button("Shut down")
                    .on_press(Message::AskPower(PowerAction::Shutdown))
                    .style(button::danger),
                button("Return to boot manager")
                    .on_press(Message::AskPower(PowerAction::BootManager))
                    .style(button::text),
            ]
            .spacing(10)
        ]
        .spacing(8);
        if let Some(action) = &self.pending_power {
            col = col.push(text(format!("Confirm {}?", power_label(action))));
            col = col.push(
                row![
                    button("Confirm")
                        .on_press(Message::ConfirmPower)
                        .style(button::danger),
                    button("Cancel")
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

fn storage(snapshot: &SystemSnapshot) -> Column<'_, Message> {
    let mut col = column![
        heading("Storage"),
        text("Free space is shown only for filesystems that are already mounted.").style(muted),
    ]
    .spacing(8);
    if snapshot.disks.is_empty() {
        col = col.push(text("No disks reported."));
    }
    for disk in &snapshot.disks {
        col = col.push(text(format!(
            "{}  {}  {}  {}  {}",
            disk.device_path,
            disk.identity.model,
            bytes(disk.identity.size_bytes),
            bus_label(disk.identity.bus),
            disk.health
        )));
        col = col.push(text(format!("id {}", disk.identity.stable_id)).style(muted));
        for part in &disk.partitions {
            let avail = part
                .fs_avail_bytes
                .map(bytes)
                .unwrap_or_else(|| "n/a".to_string());
            col = col.push(text(format!(
                "    {}  {}  {}  {}  mounted {}  free {}  {}{}",
                part.device_path,
                part.fstype.as_deref().unwrap_or("unknown"),
                bytes(part.size_bytes),
                protection_label(part.protection),
                part.mountpoint.as_deref().unwrap_or("no"),
                avail,
                part.partuuid.as_deref().unwrap_or(""),
                if part.encrypted { "  encrypted" } else { "" }
            )));
        }
    }
    col
}

fn operating_systems(snapshot: &SystemSnapshot) -> Column<'_, Message> {
    let mut col = column![
        heading("Installed operating systems"),
        text("Identities come from partition types and superblocks. Nothing is mounted to discover them.")
            .style(muted),
    ]
    .spacing(8);
    if snapshot.installed_os.is_empty() {
        col = col.push(text("No installed system was inferred."));
    }
    for os in &snapshot.installed_os {
        col = col.push(text(format!(
            "{}  {}  {}",
            os.display_name,
            os_family_label(os.os_family),
            os.evidence
        )));
        col = col.push(text(format!("target {}", os.id)).style(muted));
        col = col.push(
            button("Request this target on the next boot")
                .on_press(Message::AskPower(PowerAction::BootTarget {
                    target_id: os.id.clone(),
                }))
                .style(button::text),
        );
    }
    col
}

fn hardware(info: &HardwareInfo, snapshot: &SystemSnapshot) -> Column<'static, Message> {
    let gpus = if info.gpus.is_empty() {
        "none reported".to_string()
    } else {
        info.gpus.join(", ")
    };
    column![
        heading("Hardware"),
        fact("Vendor", &info.vendor),
        fact("Product", &info.product),
        fact("CPU", &info.cpu_model),
        fact("Cores", info.cores.to_string()),
        fact("Architecture", &info.arch),
        fact("GPUs", &gpus),
        fact("Memory", memory_line(snapshot)),
        fact("Kernel", &snapshot.kernel),
    ]
    .spacing(8)
}

fn network(snapshot: &SystemSnapshot) -> Column<'_, Message> {
    let mut col = column![
        heading("Network"),
        text("Ethernet and Wi-Fi use DHCP when a link is up. No network is required to boot.")
            .style(muted),
    ]
    .spacing(8);
    if snapshot.nics.is_empty() {
        col = col.push(text("No interfaces reported."));
    }
    for nic in &snapshot.nics {
        let kind = if nic.wireless { "wifi" } else { "ethernet" };
        let addrs = if nic.addresses.is_empty() {
            "no address".to_string()
        } else {
            nic.addresses.join(", ")
        };
        col = col.push(text(format!(
            "{}  {}  {}  {}  {}",
            nic.name,
            kind,
            nic.state,
            nic.mac.as_deref().unwrap_or(""),
            addrs
        )));
    }
    col
}

fn nav_button<'a>(label: &'a str, page: Page, current: Page) -> Element<'a, Message> {
    let control = button(text(label))
        .on_press(Message::Navigate(page))
        .width(Fill);
    if page == current {
        control.style(button::primary).into()
    } else {
        control.style(button::text).into()
    }
}

fn heading(value: &str) -> iced::widget::Text<'_> {
    text(value).size(28)
}

fn load_snapshot() -> Task<Message> {
    Task::perform(
        async { transact(&ClientRequest::Snapshot) },
        Message::Snapshot,
    )
}

fn fact(label: &'static str, value: impl AsRef<str>) -> Element<'static, Message> {
    row![text(label).width(160), text(value.as_ref().to_string())]
        .spacing(8)
        .into()
}

fn muted(theme: &Theme) -> iced::widget::text::Style {
    let _ = theme;
    iced::widget::text::Style {
        color: Some(theme::MUTED),
    }
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
