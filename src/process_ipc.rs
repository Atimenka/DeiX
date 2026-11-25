//! Bounded, non-blocking process mailboxes. The syscall layer supplies the
//! authenticated sender and checks capabilities; no user pointer is held here.
//! Dinit does not yet use this transport.

use crate::spinlock::IrqSpinLock;

const CAPACITY: usize = 64;
const PER_RECEIVER: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message { pub sender: u32, pub receiver: u32, pub tag: u64, pub a: u64, pub b: u64 }
const EMPTY: Message = Message { sender: 0, receiver: 0, tag: 0, a: 0, b: 0 };

struct Queue { items: [Message; CAPACITY], len: usize }
impl Queue {
    const fn new() -> Self { Self { items: [EMPTY; CAPACITY], len: 0 } }
    fn send(&mut self, msg: Message) -> bool {
        if self.len == CAPACITY || self.items[..self.len].iter().filter(|m| m.receiver == msg.receiver).count() >= PER_RECEIVER { return false; }
        self.items[self.len] = msg;
        self.len += 1;
        true
    }
    fn receive(&mut self, receiver: u32) -> Option<Message> {
        let index = self.items[..self.len].iter().position(|m| m.receiver == receiver)?;
        let msg = self.items[index];
        self.items.copy_within(index + 1..self.len, index);
        self.len -= 1;
        self.items[self.len] = EMPTY;
        Some(msg)
    }
    fn purge(&mut self, receiver: u32) {
        while self.receive(receiver).is_some() {}
    }
}
static QUEUE: IrqSpinLock<Queue> = IrqSpinLock::new(Queue::new());
pub fn send(msg: Message) -> bool { QUEUE.lock().send(msg) }
pub fn receive(receiver: u32) -> Option<Message> { QUEUE.lock().receive(receiver) }
pub fn purge(receiver: u32) { QUEUE.lock().purge(receiver); }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifo_is_per_receiver_and_purge_is_targeted() {
        let mut q = Queue::new();
        let msg = |sender, receiver, tag| Message { sender, receiver, tag, a: 0, b: 0 };
        assert!(q.send(msg(1, 2, 10))); assert!(q.send(msg(1, 3, 20))); assert!(q.send(msg(4, 2, 30)));
        assert_eq!(q.receive(2).unwrap().tag, 10);
        q.purge(3);
        assert_eq!(q.receive(2).unwrap().sender, 4);
        assert_eq!(q.receive(3), None);
    }
    #[test]
    fn enforces_per_receiver_and_global_limits() {
        let mut q = Queue::new();
        for receiver in 1..=8 { for _ in 0..PER_RECEIVER { assert!(q.send(Message { receiver, ..EMPTY })); } }
        assert!(!q.send(Message { receiver: 9, ..EMPTY }));
        q.purge(1);
        for _ in 0..PER_RECEIVER { assert!(q.send(Message { receiver: 9, ..EMPTY })); }
        assert!(!q.send(Message { receiver: 9, ..EMPTY }));
    }
}
