#![allow(dead_code)]

/// A bounded ring buffer with O(1) push and iteration.
pub struct RingBuffer<T, const N: usize> {
    buf: [Option<T>; N],
    head: usize,
    len: usize,
}

impl<T, const N: usize> RingBuffer<T, N> {
    pub fn new() -> Self {
        Self { buf: [const { None }; N], head: 0, len: 0 }
    }

    pub fn push(&mut self, item: T) {
        let idx = (self.head + self.len) % N;
        if self.len == N {
            self.buf[self.head] = None;
            self.head = (self.head + 1) % N;
        } else {
            self.len += 1;
        }
        self.buf[idx] = Some(item);
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        (0..self.len).filter_map(move |i| {
            self.buf[(self.head + i) % N].as_ref()
        })
    }
}
