//! Nano-Core DAG — граф зависимостей boot-этапов и служб.
//!
//! Каждый этап (stage) имеет имя и список зависимостей — имена этапов,
//! которые должны завершиться до него. Движок:
//! - сохраняет записи этапов вместе с временем начала и конца;
//! - вычисляет «волны» (waves) — уровни топологического порядка:
//!   все этапы одной волны независимы и могут идти параллельно;
//! - обнаруживает циклы в графе (зависимости не могут закольцовываться);
//! - разрешает имя → волну для отчётов.
//!
//! Движок строго детерминированный и не выполняет код сам —
//! исполнение этапов производится orchestrator'ом (в boot-цепочке
//! в порядке, вычисленном из волн).

use alloc::string::String;
use alloc::vec::Vec;

/// Запись об одном этапе.
#[derive(Debug, Clone)]
pub struct StageRecord {
    /// Имя этапа, например "interrupts" или "hb-monitor".
    pub name: String,
    /// Имена этапов, от которых зависит этот этап.
    pub deps: Vec<String>,
    /// Волна (топологический уровень). 0 — без зависимостей.
    pub wave: u32,
    /// Время старта в ms от загрузки (0 — не запущен).
    pub started_ms: u64,
    /// Время завершения в ms (0 — не завершен).
    pub done_ms: u64,
    /// Признак успешного завершения.
    pub ok: bool,
}

impl StageRecord {
    pub fn new(name: impl Into<String>, deps: Vec<String>) -> Self {
        StageRecord {
            name: name.into(),
            deps,
            wave: 0,
            started_ms: 0,
            done_ms: 0,
            ok: false,
        }
    }

    /// Длительность этапа в ms (0, если не запущен или не завершен).
    #[allow(dead_code)] // API-поверхность для отчётов
    pub fn duration_ms(&self) -> u64 {
        if self.started_ms > 0 && self.done_ms >= self.started_ms {
            self.done_ms - self.started_ms
        } else {
            0
        }
    }
}

/// Граф зависимостей этапов.
#[derive(Default)]
pub struct Dag {
    stages: Vec<StageRecord>,
}

impl Dag {
    pub const fn new() -> Self {
        Dag { stages: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    pub fn stages(&self) -> &[StageRecord] {
        &self.stages
    }

    /// Mutable accessor: обновляет записи этапов (старт/окончание/результат).
    pub fn stages_mut(&mut self) -> &mut [StageRecord] {
        &mut self.stages
    }

    #[allow(dead_code)] // API-поверхность для отчётов
    pub fn get(&self, name: &str) -> Option<&StageRecord> {
        self.stages.iter().find(|s| s.name == name)
    }

    /// Добавляет этап с зависимостями. Возвращает false, если найден цикл.
    pub fn add(&mut self, name: impl Into<String>, deps: &[&str]) -> bool {
        let name = name.into();
        if self.get(&name).is_some() {
            return true; // уже есть — обновление пропускаем
        }
        // Проверка циклов: DFS по зависимостям с отметкой «в работе».
        let mut on_stack = alloc::collections::BTreeSet::new();
        if !self.detect_cycle(&name, &mut on_stack) {
            // Цикл — этап не добавляем.
            return false;
        }
        self.stages.push(StageRecord::new(name, deps.iter().map(|d| String::from(*d)).collect()));
        self.recompute_waves();
        true
    }

    fn detect_cycle(&self, name: &str, on_stack: &mut alloc::collections::BTreeSet<String>) -> bool {
        if on_stack.contains(name) {
            return false; // закольцовано
        }
        let Some(stage) = self.get(name) else {
            return true; // неизвестная зависимость не цикл
        };
        on_stack.insert(String::from(name));
        for dep in &stage.deps {
            if !self.detect_cycle(dep, on_stack) {
                on_stack.remove(name);
                return false;
            }
        }
        on_stack.remove(name);
        true
    }

    /// Пересчитывает волны: wave(stage) = 1 + max(wave(deps)).
    fn recompute_waves(&mut self) {
        // Сначала вычисляем все волны в отдельной таблице (только
        // неисключающая итерация), потом записываем обратно.
        let mut waves: alloc::collections::BTreeMap<String, u32> = alloc::collections::BTreeMap::new();
        let mut computed: Vec<(String, u32)> = Vec::with_capacity(self.stages.len());
        for stage in &self.stages {
            let w = self.wave_of(&stage.name, &mut waves);
            computed.push((stage.name.clone(), w));
        }
        for (name, w) in computed {
            if let Some(rec) = self.stages.iter_mut().find(|s| s.name == name) {
                rec.wave = w;
            }
        }
    }

    fn wave_of(
        &self,
        name: &str,
        memo: &mut alloc::collections::BTreeMap<String, u32>,
    ) -> u32 {
        if let Some(w) = memo.get(name) {
            return *w;
        }
        let Some(stage) = self.get(name) else {
            return 0;
        };
        let mut max = 0u32;
        for dep in &stage.deps {
            max = max.max(self.wave_of(dep, memo) + 1);
        }
        memo.insert(String::from(name), max);
        max
    }

    /// Порядок выполнения: этапы в порядке возрастания волн (стабильный
    /// внутри волны — по добавлению). Это и есть план загрузки.
    pub fn execution_order(&self) -> Vec<&StageRecord> {
        let mut sorted: Vec<&StageRecord> = self.stages.iter().collect();
        sorted.sort_by_key(|s| s.wave);
        sorted
    }

    /// Кол-во волн в графе.
    pub fn wave_count(&self) -> u32 {
        self.stages.iter().map(|s| s.wave).max().unwrap_or(0) + 1
    }
}
