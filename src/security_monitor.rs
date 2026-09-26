// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// security_monitor — эвристический демон анализа угроз (Ring 3 / Ring 0 мост):
// перехват ransomware-активности, инъекций кода в системные пути и высылка SIGKILL.
// no_std-совместимо: только core/alloc (BTreeMap, String, Vec), вывод — crate::println!.

#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Порог принятия решения о ликвидации угрозы (жёстко 0.85 по ТЗ).
pub const DEFAULT_RISK_THRESHOLD: f32 = 0.85;

/// Порог частоты sys_write, выше которого запись считается лавинообразной.
pub const RANSOMWARE_WRITE_FREQUENCY_THRESHOLD: u64 = 150;

/// Порог энтропии: выше 0.75 — зашифрованные/сжатые данные.
pub const RANSOMWARE_ENTROPY_THRESHOLD: f32 = 0.75;

/// Префиксы системных путей, запретных для модификации из Ring 3.
pub const SYSTEM_TARGET_PREFIXES: [&str; 4] = [
    "/kernel",
    "/system",
    "/init_boot",
    "/dev/block/by-name",
];

/// POSIX-константа SIGKILL (9) — безусловное уничтожение процесса.
pub const SIGKILL: i32 = 9;

/// Объект, инкапсулирующий низкоуровневые метрики системного вызова,
/// перехваченного ядром и переданного супервизору.
#[derive(Debug, Clone, PartialEq)]
pub struct SecurityEvent {
    pub pid: u32,
    pub action: String,
    pub target_path: String,
    pub syscall_frequency: u64,
    pub entropy_score: f32,
}

impl SecurityEvent {
    pub fn new(
        pid: u32,
        action: &str,
        target_path: &str,
        syscall_frequency: u64,
        entropy_score: f32,
    ) -> SecurityEvent {
        SecurityEvent {
            pid,
            action: action.to_string(),
            target_path: target_path.to_string(),
            syscall_frequency,
            entropy_score,
        }
    }

    /// Предикат: направлено ли событие на системный путь ядра.
    pub fn is_system_target(&self) -> bool {
        let mut matched: bool = false;
        for prefix in SYSTEM_TARGET_PREFIXES.iter() {
            if self.target_path.starts_with(prefix) {
                matched = true;
                break;
            }
        }
        matched
    }
}

/// Классификация вектора угрозы, выявленного эвристическим движком.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreatVector {
    RansomwareEncryption,
    CodeInjection,
    GenericAnomaly,
    NoneDetected,
}

/// Степень опасности вердикта (упорядочена: Safe < Suspicious < Critical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreatSeverity {
    Safe,
    Suspicious,
    Critical,
}

/// Полный вердикт эвристического анализа.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreatVerdict {
    pub severity: ThreatSeverity,
    pub risk_score: f32,
    pub vectors: Vec<ThreatVector>,
}

/// Сигнал, возвращаемый демоном планировщику ядра: SIGKILL по PID.
#[derive(Debug, Clone, PartialEq)]
pub struct KillSignal {
    pub pid: u32,
    pub signal: i32,
    pub reason: String,
}

impl KillSignal {
    pub fn new(pid: u32, signal: i32, reason: &str) -> KillSignal {
        KillSignal {
            pid,
            signal,
            reason: reason.to_string(),
        }
    }

    pub fn describe(&self) -> String {
        format!("SIGKILL({}) -> PID {} :: {}", self.signal, self.pid, self.reason)
    }
}

/// Поведенческий профиль процесса, ведущийся демоном между событиями.
#[derive(Debug, Clone)]
pub struct ProcessProfile {
    pub pid: u32,
    pub events_seen: u64,
    pub last_risk_score: f32,
    pub terminated: bool,
}

impl ProcessProfile {
    pub fn new(pid: u32) -> ProcessProfile {
        ProcessProfile {
            pid,
            events_seen: 0,
            last_risk_score: 0.0,
            terminated: false,
        }
    }

    pub fn record(&mut self) {
        self.events_seen += 1;
    }
}

/// Демон эвристического анализа — ядро Ring 3 / Ring 0 супервизора.
pub struct HeuristicAnalysisEngine {
    pub monitor_id: String,
    pub risk_threshold: f32,
    pub profiles: BTreeMap<u32, ProcessProfile>,
    pub kill_signals: Vec<KillSignal>,
    pub events_analyzed: u64,
    pub critical_alerts: u64,
}

impl HeuristicAnalysisEngine {
    pub fn new(monitor_id: &str) -> HeuristicAnalysisEngine {
        HeuristicAnalysisEngine::with_threshold(monitor_id, DEFAULT_RISK_THRESHOLD)
    }

    pub fn with_threshold(monitor_id: &str, risk_threshold: f32) -> HeuristicAnalysisEngine {
        HeuristicAnalysisEngine {
            monitor_id: monitor_id.to_string(),
            risk_threshold,
            profiles: BTreeMap::new(),
            kill_signals: Vec::new(),
            events_analyzed: 0,
            critical_alerts: 0,
        }
    }

    /// Регистрация процесса в таблице наблюдения (идемпотентно).
    pub fn register_process(&mut self, pid: u32) {
        if !self.profiles.contains_key(&pid) {
            self.profiles.insert(pid, ProcessProfile::new(pid));
        }
    }

    /// МНОГОФАКТОРНЫЙ АЛГОРИТМ ВЫЧИСЛЕНИЯ КОЭФФИЦИЕНТА ОПАСНОСТИ.
    pub fn compute_verdict(&self, event: &SecurityEvent) -> ThreatVerdict {
        let entropy: f32 = event.entropy_score.clamp(0.0, 1.0);

        let (risk, mut vectors): (f32, Vec<ThreatVector>) = match event.action.as_str() {
            // --- RANSOMWARE-ПАТТЕРН -------------------------------------
            "sys_write" => {
                if event.syscall_frequency > RANSOMWARE_WRITE_FREQUENCY_THRESHOLD
                    && entropy > RANSOMWARE_ENTROPY_THRESHOLD
                {
                    let freq_factor: f32 =
                        ((event.syscall_frequency - RANSOMWARE_WRITE_FREQUENCY_THRESHOLD) as f32)
                            / 300.0;
                    let freq_factor: f32 = freq_factor.min(1.0);
                    let risk: f32 = 0.5 * freq_factor + 0.5 * entropy;
                    (risk, vec![ThreatVector::RansomwareEncryption])
                } else {
                    let freq_norm: f32 = (event.syscall_frequency.min(100) as f32) / 100.0;
                    let risk: f32 = 0.05 * freq_norm + 0.20 * entropy;
                    let mut local_vectors: Vec<ThreatVector> = Vec::new();
                    if risk > self.risk_threshold * 0.5 {
                        local_vectors.push(ThreatVector::GenericAnomaly);
                    }
                    (risk, local_vectors)
                }
            }
            // --- CODE INJECTION -----------------------------------------
            "sys_mmap" | "sys_ptrace" => {
                if event.is_system_target() {
                    (1.0, vec![ThreatVector::CodeInjection])
                } else {
                    let risk: f32 = 0.4 + 0.3 * entropy;
                    (risk, vec![ThreatVector::GenericAnomaly])
                }
            }
            // --- ПРОЧИЕ ВЫЗОВЫ ------------------------------------------
            _ => {
                let freq_norm: f32 = (event.syscall_frequency.min(200) as f32) / 200.0;
                let risk: f32 = 0.15 * freq_norm + 0.25 * entropy;
                let mut local_vectors: Vec<ThreatVector> = Vec::new();
                if risk > self.risk_threshold * 0.5 {
                    local_vectors.push(ThreatVector::GenericAnomaly);
                }
                (risk, local_vectors)
            }
        };

        let risk: f32 = risk.clamp(0.0, 1.0);

        let severity: ThreatSeverity = match risk {
            r if r > self.risk_threshold => ThreatSeverity::Critical,
            r if r > self.risk_threshold * 0.5 => ThreatSeverity::Suspicious,
            _ => ThreatSeverity::Safe,
        };

        if vectors.is_empty() {
            match severity {
                ThreatSeverity::Safe => vectors.push(ThreatVector::NoneDetected),
                ThreatSeverity::Suspicious | ThreatSeverity::Critical => {
                    vectors.push(ThreatVector::GenericAnomaly)
                }
            }
        }

        ThreatVerdict {
            severity,
            risk_score: risk,
            vectors,
        }
    }

    /// Предикат принятия решения: true, если уровень критический.
    pub fn analyze_event(&self, event: &SecurityEvent) -> bool {
        let verdict: ThreatVerdict = self.compute_verdict(event);
        verdict.severity == ThreatSeverity::Critical
    }

    /// Полный цикл обработки: анализ + протокол реагирования.
    pub fn process_event(&mut self, event: &SecurityEvent) -> bool {
        self.events_analyzed += 1;
        self.register_process(event.pid);
        if let Some(profile) = self.profiles.get_mut(&event.pid) {
            profile.record();
        }

        let verdict: ThreatVerdict = self.compute_verdict(event);
        if verdict.severity == ThreatSeverity::Critical {
            self.critical_alerts += 1;
            let reason: String = self.format_reason(event, &verdict);
            self.kill_signals.push(KillSignal::new(event.pid, SIGKILL, &reason));
            if let Some(profile) = self.profiles.get_mut(&event.pid) {
                profile.terminated = true;
                profile.last_risk_score = verdict.risk_score;
            }
            true
        } else {
            false
        }
    }

    /// Формирование человекочитаемого обоснования kill-сигнала.
    fn format_reason(&self, event: &SecurityEvent, verdict: &ThreatVerdict) -> String {
        let vector_names: Vec<&str> = verdict
            .vectors
            .iter()
            .map(|v| match v {
                ThreatVector::RansomwareEncryption => "ransomware-encryption",
                ThreatVector::CodeInjection => "code-injection",
                ThreatVector::GenericAnomaly => "generic-anomaly",
                ThreatVector::NoneDetected => "none",
            })
            .collect();
        format!(
            "pid {} {}: risk {:.3} > threshold {:.3}; vectors: {}",
            event.pid,
            event.action,
            verdict.risk_score,
            self.risk_threshold,
            vector_names.join(", ")
        )
    }
}

/// Самоконтроль эвристического монитора при загрузке ядра:
/// проверяет легитимную запись (SAFE) и симулированную рансомвару (SIGKILL).
pub fn boot_selfcheck() {
    crate::println!("  [secmon] Starting Heuristic Security Monitor selfcheck...");
    let mut engine = HeuristicAnalysisEngine::new("deix_heuristic_core_v1");

    // 1. Легитимная запись текстового редактора
    let benign = SecurityEvent::new(1001, "sys_write", "/userdata/editor/notes.log", 10, 0.22);
    let benign_verdict = engine.compute_verdict(&benign);
    if benign_verdict.severity != ThreatSeverity::Safe {
        crate::println!("  [secmon] Warning: unexpected verdict for benign event");
    } else {
        crate::println!("  [secmon] Legitimate write: SAFE (risk {:.3})", benign_verdict.risk_score);
    }

    // 2. Симулированная атака программы-вымогателя (высокая частота + высокая энтропия)
    let attack = SecurityEvent::new(666, "sys_write", "/userdata/private/data.bin", 450, 0.92);
    let attack_flagged = engine.process_event(&attack);
    if attack_flagged && !engine.kill_signals.is_empty() {
        let sig = &engine.kill_signals[0];
        crate::println!("  [secmon] Ransomware detected: SIGKILL -> PID {} (risk > 0.85)", sig.pid);
        crate::println!("  [secmon] Heuristic Security Monitor: ALL CHECKS PASSED");
    } else {
        crate::println!("  [secmon] ERROR: Ransomware attack was not intercepted!");
    }
}
