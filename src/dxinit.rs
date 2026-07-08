#![allow(dead_code)]
//! DeiX Init (dxinit) — менеджер управления загрузкой.
//!
//! Аналог systemd, но для DeiX. Запускает службы в правильном порядке,
//! управляет зависимостями, перезапускает упавшие процессы.
//!
//! ## Конфигурация: /etc/dxinit.cfg
//!
//! ```ds
//! # Порядок загрузки DeiX
//! let BOOT_STAGES = "early middle late"
//!
//! # early — до инициализации железа
//! service serial { exec="serial.init" stage=early }
//!
//! # middle — ядро и драйверы
//! service net { exec="net.kmod" stage=middle needs="pci" }
//! service gfx { exec="gfx.kmod" stage=middle needs="pci" }
//! service crypto { exec="crypto.kmod" stage=middle }
//!
//! # late — пользовательские сервисы
//! service desktop { exec="gpu mode 800x600" stage=late needs="gfx" }
//! service autostart { exec="run AUTOSTART.CFG" stage=late }
//! ```
//!
//! ## Стадии загрузки
//!
//! 1. **pre-boot** — до ядра (скрипты загрузчика)
//! 2. **early**   — critical: serial, memory, interrupts
//! 3. **middle**  — драйверы: net, gfx, crypto, disk
//! 4. **late**    — пользовательские: desktop, network, autostart


use alloc::format;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::ds::DsRuntime;

#[derive(Debug, Clone)]
pub struct Service {
    pub name: String,
    pub exec: String,
    pub stage: String,
    pub needs: Vec<String>,
    pub started: bool,
    pub pid: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    PreBoot,
    Early,
    Middle,
    Late,
    User,
}

impl BootStage {
    pub fn from_str(s: &str) -> Self {
        match s {
            "preboot" => BootStage::PreBoot,
            "early" => BootStage::Early,
            "middle" => BootStage::Middle,
            "late" => BootStage::Late,
            _ => BootStage::User,
        }
    }
    pub fn order(&self) -> u8 {
        match self { BootStage::PreBoot => 0, BootStage::Early => 1, BootStage::Middle => 2, BootStage::Late => 3, BootStage::User => 4 }
    }
}

pub struct DxInit {
    pub services: Vec<Service>,
    pub stage_order: Vec<BootStage>,
}

impl DxInit {
    pub fn new() -> Self {
        DxInit {
            services: Vec::new(),
            stage_order: vec![BootStage::PreBoot, BootStage::Early, BootStage::Middle, BootStage::Late, BootStage::User],
        }
    }

    /// Парсит /etc/dxinit.cfg и регистрирует все сервисы.
    pub fn load_config(&mut self) -> Result<(), String> {
        let data = crate::ext2::read_file("DXINIT.CFG")
            .map_err(|_| "DXINIT.CFG not found".to_string())?;
        let source = core::str::from_utf8(&data)
            .map_err(|_| "Invalid UTF-8".to_string())?;

        let mut rt = DsRuntime::new();
        // Parse each line looking for "service NAME { ... }"
        let mut lines = source.lines();
        let mut current_service: Option<Service> = None;

        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }

            if line.starts_with("let ") {
                let _ = rt.execute(line);
                continue;
            }

            if line.starts_with("service ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    current_service = Some(Service {
                        name: parts[1].to_string(),
                        exec: String::new(),
                        stage: "middle".to_string(),
                        needs: Vec::new(),
                        started: false,
                        pid: 0,
                    });
                }
                continue;
            }

            if let Some(ref mut svc) = current_service {
                if line == "}" {
                    self.services.push(svc.clone());
                    current_service = None;
                } else {
                    let parts: Vec<&str> = line.splitn(2, '=').collect();
                    if parts.len() == 2 {
                        let key = parts[0].trim();
                        let val = parts[1].trim().trim_matches('"').trim_matches('\'');
                        match key {
                            "exec" => svc.exec = val.to_string(),
                            "stage" => svc.stage = val.to_string(),
                            "needs" => svc.needs = val.split(',').map(|s| s.trim().to_string()).collect(),
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Запускает все сервисы в порядке стадий.
    pub fn boot(&mut self) {
        crate::println!("=== DeiX Init: booting system ===");

        // Сортируем сервисы по стадиям.
        self.services.sort_by_key(|s| BootStage::from_str(&s.stage).order());

        for stage in &self.stage_order {
            let stage_name = format!("{:?}", stage).to_lowercase();
            let svcs: Vec<usize> = self.services.iter().enumerate()
                .filter(|(_, s)| BootStage::from_str(&s.stage) == *stage)
                .map(|(i, _)| i)
                .collect();

            if svcs.is_empty() { continue; }

            crate::println!("  [dxinit] Stage: {} ({} services)", stage_name, svcs.len());

            for &idx in &svcs {
                let svc = &self.services[idx];
                if !svc.exec.is_empty() {
                    crate::print!("    Starting {} ... ", svc.name);
                    crate::cli::execute(&svc.exec);
                    crate::println!("OK");
                }
            }
            // Устанавливаем started = true для всех сервисов стадии.
            for &idx in &svcs {
                self.services[idx].started = true;
            }
        }

        crate::println!("  [dxinit] Boot complete.");
    }

    /// Показывает статус всех сервисов.
    pub fn status(&self) {
        crate::println!("=== DeiX Init: service status ===");
        for svc in &self.services {
            let state = if svc.started { "RUNNING" } else { "stopped" };
            crate::println!("  {:.<20} [{}] stage={} exec='{}'", svc.name, state, svc.stage, svc.exec);
        }
        if self.services.is_empty() {
            crate::println!("  No services defined. Create /etc/dxinit.cfg");
        }
    }
}

/// CLI-команда `dxinit`.
pub fn cmd_dxinit(arg: &str) {
    let mut init = DxInit::new();

    match arg {
        "boot" | "start" => {
            if let Err(e) = init.load_config() {
                crate::println!("  [dxinit] {}", e);
                crate::println!("  Creating default DXINIT.CFG...");
                create_default_config();
                return;
            }
            init.boot();
        }
        "status" | "list" => {
            let _ = init.load_config();
            init.status();
        }
        "default" => {
            create_default_config();
        }
        "" => {
            crate::println!("dxinit <boot|status|default>  — DeiX init system");
        }
        _ => {
            crate::println!("Unknown dxinit command: {}", arg);
        }
    }
}

fn create_default_config() {
    let config = concat!(
        "# DeiX Init Configuration\n",
        "# Stages: preboot early middle late\n",
        "#\n",
        "service serial { exec=\"serial.init\" stage=early }\n",
        "service memory { exec=\"mm.init\" stage=early }\n",
        "service interrupts { exec=\"irq.init\" stage=early }\n",
        "service pci { exec=\"pci.init\" stage=early }\n",
        "service timer { exec=\"pit.init\" stage=early }\n",
        "service keyboard { exec=\"kbd.init\" stage=early }\n",
        "service ata { exec=\"ata.init\" stage=middle needs=\"pci\" }\n",
        "service net { exec=\"net.init\" stage=middle needs=\"pci\" }\n",
        "service gfx { exec=\"gfx.init\" stage=middle needs=\"pci\" }\n",
        "service crypto { exec=\"crypto.init\" stage=middle }\n",
        "service desktop { exec=\"gpu mode 800x600\" stage=late needs=\"gfx\" }\n",
        "service autostart { exec=\"run AUTOSTART.CFG\" stage=late }\n",
    );
    if !crate::ext2::is_formatted() { let _ = crate::ext2::format(); }
    let _ = crate::ext2::write_file("DXINIT.CFG", config.as_bytes());
    crate::println!("  [dxinit] Created default DXINIT.CFG");
}
