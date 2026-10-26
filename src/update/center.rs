//! Состояние окна «Updates» рабочего стола.
//!
//! Окно ходит по тем же примитивам, что и команда `update`: проверка подписи
//! манифеста, пошаговая загрузка, запрос на применение в управляющем секторе.
//! Блокирующие сетевые вызовы выполняются по одному за кадр (`step`), поэтому
//! рабочий стол между кусками перерисовывается и остаётся отзывчивым.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::{
    cancel_staged, check_target, clear_staged, clip, download_precheck, ensure_newer, fetch_offer,
    installed_build, is_enabled, is_staged, load_staged, pending, request_apply, Download, Offer,
};
use crate::ui::notify::{self, Kind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Ready,
    Scheduled,
    Failed,
}

pub struct Center {
    stage: Stage,
    /// Первый вызов `step` после смены стадии только просит перерисовку:
    /// блокирующая работа не должна начаться раньше, чем на экране появится
    /// новая надпись.
    wait_frame: bool,
    confirm: bool,
    blocked: Option<String>,
    info: Option<String>,
    error: Option<String>,
    offer: Option<Offer>,
    download: Option<Download>,
    staged: Option<(String, u64)>,
}

impl Center {
    pub fn new() -> Center {
        let mut c = Center {
            stage: Stage::Idle,
            wait_frame: false,
            confirm: false,
            blocked: None,
            info: None,
            error: None,
            offer: None,
            download: None,
            staged: None,
        };
        if !is_enabled() {
            c.blocked = Some(String::from(
                "This build has no update key, so updates are disabled (docs/UPDATE.md).",
            ));
        } else if let Err(e) = check_target() {
            c.blocked = Some(e);
        }
        if c.blocked.is_some() {
            return c;
        }
        if pending() {
            c.stage = Stage::Scheduled;
        } else if let Ok(s) = load_staged() {
            let installed = installed_build();
            if installed == 0 || s.manifest.build > installed {
                c.staged = Some((s.manifest.version.clone(), s.manifest.build));
                c.stage = Stage::Ready;
            }
        }
        c
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn confirming(&self) -> bool {
        self.confirm
    }

    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    pub fn info(&self) -> Option<&str> {
        self.info.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn installed_line(&self) -> String {
        match installed_build() {
            0 => String::from("Build: unknown"),
            n => format!("Build: {}", n),
        }
    }

    /// Строки о найденном (Available) или скачанном (Ready) выпуске.
    pub fn release_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.stage == Stage::Available {
            if let Some(o) = self.offer.as_ref() {
                let m = &o.signed.manifest;
                lines.push(format!("Version: {} (build {}, {})", m.version, m.build, m.channel));
                lines.push(format!("Size: {} KiB in {} parts", m.payload_size / 1024, o.parts));
                if !m.notes.is_empty() {
                    lines.push(format!("Notes: {}", clip(&m.notes, 48)));
                }
            }
        } else if let Some((version, build)) = self.staged.as_ref() {
            lines.push(format!("Version: {} (build {})", version, build));
            lines.push(String::from("The manifest signature is valid."));
        }
        lines
    }

    pub fn percent(&self) -> u32 {
        match (self.offer.as_ref(), self.download.as_ref()) {
            (Some(o), Some(d)) if o.signed.manifest.payload_size > 0 => {
                (d.downloaded_bytes() as u64 * 100 / o.signed.manifest.payload_size as u64) as u32
            }
            _ => 0,
        }
    }

    pub fn progress_line(&self) -> String {
        match (self.offer.as_ref(), self.download.as_ref()) {
            (Some(o), Some(d)) => format!(
                "{} of {} KiB (part {} of {})",
                d.downloaded_bytes() / 1024,
                o.signed.manifest.payload_size / 1024,
                d.next,
                o.parts
            ),
            _ => String::new(),
        }
    }

    pub fn primary_label(&self) -> Option<&'static str> {
        if self.blocked.is_some() {
            return None;
        }
        match self.stage {
            Stage::Idle => Some("Check now"),
            Stage::UpToDate => Some("Check again"),
            Stage::Failed if pending() => Some("Back"),
            Stage::Failed => Some("Check again"),
            Stage::Available => Some("Download"),
            Stage::Ready if self.confirm => Some("Confirm and restart"),
            Stage::Ready => Some("Install and restart"),
            Stage::Scheduled => Some("Restart now"),
            Stage::Checking | Stage::Downloading => None,
        }
    }

    pub fn secondary_label(&self) -> Option<&'static str> {
        if self.blocked.is_some() {
            return None;
        }
        match self.stage {
            Stage::Available => Some("Check again"),
            Stage::Ready if self.confirm => Some("Back"),
            Stage::Ready => Some("Discard"),
            Stage::Scheduled => Some("Cancel"),
            _ => None,
        }
    }

    pub fn primary(&mut self) {
        if self.primary_label().is_none() {
            return;
        }
        match self.stage {
            Stage::Idle | Stage::UpToDate => self.begin_check(),
            Stage::Failed => {
                if pending() {
                    self.error = None;
                    self.stage = Stage::Scheduled;
                } else {
                    self.begin_check();
                }
            }
            Stage::Available => self.begin_download(),
            Stage::Ready => {
                if self.confirm {
                    self.apply();
                } else {
                    self.confirm = true;
                }
            }
            Stage::Scheduled => crate::cli::cmd_reboot(),
            Stage::Checking | Stage::Downloading => {}
        }
    }

    pub fn secondary(&mut self) {
        if self.secondary_label().is_none() {
            return;
        }
        match self.stage {
            Stage::Available => self.begin_check(),
            Stage::Ready if self.confirm => self.confirm = false,
            Stage::Ready | Stage::Scheduled => self.discard(),
            _ => {}
        }
    }

    /// Один шаг фоновой работы. `true` — состояние изменилось, нужна перерисовка.
    pub fn step(&mut self) -> bool {
        match self.stage {
            Stage::Checking | Stage::Downloading => {
                if self.wait_frame {
                    self.wait_frame = false;
                    return true;
                }
                if self.stage == Stage::Checking {
                    self.run_check();
                } else {
                    self.run_download_step();
                }
                true
            }
            _ => false,
        }
    }

    fn fail(&mut self, message: String) {
        self.error = Some(message);
        self.info = None;
        self.confirm = false;
        self.download = None;
        self.stage = Stage::Failed;
    }

    fn begin_check(&mut self) {
        self.error = None;
        self.info = None;
        self.confirm = false;
        self.offer = None;
        self.download = None;
        self.stage = Stage::Checking;
        self.wait_frame = true;
    }

    fn run_check(&mut self) {
        match fetch_offer() {
            Ok(None) => {
                self.info = Some(String::from("The server has no published update."));
                self.stage = Stage::UpToDate;
            }
            Ok(Some(offer)) => {
                let build = offer.signed.manifest.build;
                let installed = installed_build();
                if installed != 0 && build <= installed {
                    self.info = Some(format!("DeiX is up to date (build {}).", installed));
                    self.stage = Stage::UpToDate;
                    return;
                }
                match ensure_newer(&offer.signed.manifest) {
                    Ok(()) => {
                        self.offer = Some(offer);
                        self.stage = Stage::Available;
                    }
                    Err(e) => self.fail(e),
                }
            }
            Err(e) => self.fail(format!("Check failed: {}", e)),
        }
    }

    fn begin_download(&mut self) {
        let (build, version) = match self.offer.as_ref() {
            Some(o) => (o.signed.manifest.build, o.signed.manifest.version.clone()),
            None => return,
        };
        if let Err(e) = download_precheck() {
            self.fail(e);
            return;
        }
        self.error = None;
        self.info = None;
        if is_staged(build) {
            self.staged = Some((version, build));
            self.stage = Stage::Ready;
            return;
        }
        self.download = self.offer.as_ref().map(Download::new);
        self.stage = Stage::Downloading;
        self.wait_frame = true;
    }

    fn run_download_step(&mut self) {
        let (Some(offer), Some(mut download)) = (self.offer.take(), self.download.take()) else {
            self.fail(String::from("Internal error: the download state was lost."));
            return;
        };
        if let Err(e) = download.step(&offer) {
            download.stash(offer.signed.manifest.build);
            clear_staged();
            notify::push(Kind::Error, "Update download failed", &clip(&e, 90));
            self.offer = Some(offer);
            self.fail(format!("Download failed: {}", e));
            return;
        }
        if !download.done(&offer) {
            self.offer = Some(offer);
            self.download = Some(download);
            return;
        }
        let build = offer.signed.manifest.build;
        let version = offer.signed.manifest.version.clone();
        match download.finish(&offer) {
            Ok(()) => {
                notify::push(
                    Kind::Success,
                    "Update downloaded",
                    &format!("DeiX {} is ready to install.", version),
                );
                crate::sound::play_ui_background(crate::sound::UiSound::UpdateReady);
                self.staged = Some((version, build));
                self.stage = Stage::Ready;
                self.confirm = false;
                self.offer = Some(offer);
            }
            Err(e) => {
                clear_staged();
                notify::push(Kind::Error, "Update download failed", &clip(&e, 90));
                self.offer = Some(offer);
                self.fail(format!("Download failed: {}", e));
            }
        }
    }

    fn apply(&mut self) {
        self.confirm = false;
        match request_apply() {
            Ok(_) => {
                self.stage = Stage::Scheduled;
                crate::cli::cmd_reboot();
            }
            Err(e) => self.fail(e),
        }
    }

    fn discard(&mut self) {
        match cancel_staged() {
            Ok(()) => {
                self.staged = None;
                self.confirm = false;
                self.info = Some(String::from("The downloaded package was removed."));
                self.stage = Stage::Idle;
            }
            Err(e) => self.fail(e),
        }
    }
}
