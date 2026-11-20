//! Простейший спинлок без внешних зависимостей (нам не нужен crate `spin`,
//! т.к. цель — минимальное ядро с понятным кодом). У нас всего одно "ядро"
//! CPU и предсказуемая логика, поэтому примитивная реализация достаточна.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T> Sync for SpinLock<T> {}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        SpinLock {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinLockGuard { lock: self }
    }

    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinLockGuard { lock: self })
        } else {
            None
        }
    }
}

impl<'a, T> core::ops::Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> core::ops::DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

// ==================== IrqSpinLock ====================

/// Спинлок, который на время удержания отключает прерывания.
///
/// Обычный [`SpinLock`] защищает только от параллельного доступа других
/// задач. На однопроцессорной системе этого мало: если прерывание
/// приходит, пока обычный код держит лок, обработчик начинает ждать тот
/// же самый лок, а освободить его некому — владелец заморожен на время
/// работы обработчика. Получается мгновенное зависание.
///
/// `IrqSpinLock` снимает эту проблему ценой задержки прерываний на
/// время критической секции, поэтому секции под ним обязаны быть
/// короткими. Блокирующие операции (ожидание ввода-вывода, сон) из-под
/// него запрещены: пока прерывания выключены, разбудить ожидающего
/// некому.
///
/// Правило выбора:
/// * лок читается или пишется из обработчика прерывания — `IrqSpinLock`;
/// * лок используется только из обычного кода задач — `SpinLock`.
pub struct IrqSpinLock<T> {
    inner: SpinLock<T>,
}

pub struct IrqSpinLockGuard<'a, T> {
    inner: SpinLockGuard<'a, T>,
    /// Состояние флага IF до взятия лока.
    saved_flags: u64,
}

impl<T> IrqSpinLock<T> {
    pub const fn new(data: T) -> Self {
        IrqSpinLock { inner: SpinLock::new(data) }
    }

    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        let saved_flags: u64;
        unsafe {
            core::arch::asm!(
                "pushfq",
                "pop {}",
                out(reg) saved_flags,
                options(nomem, preserves_flags)
            );
            core::arch::asm!("cli", options(nomem, nostack));
        }
        IrqSpinLockGuard {
            inner: self.inner.lock(),
            saved_flags,
        }
    }
}

impl<'a, T> core::ops::Deref for IrqSpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<'a, T> core::ops::DerefMut for IrqSpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<'a, T> Drop for IrqSpinLockGuard<'a, T> {
    fn drop(&mut self) {
        const IF_BIT: u64 = 1 << 9;
        let restore = self.saved_flags & IF_BIT != 0;
        // Лок обязан быть свободен ДО восстановления прерываний: иначе
        // обработчик, вошедший сразу после `sti`, увидит занятым лок,
        // который уже никто не держит. Поле `inner` снимется ещё раз
        // при выходе из Drop — повторная запись `false` идемпотентна.
        self.inner.lock.locked.store(false, Ordering::Release);
        if restore {
            unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
        }
    }
}
