// Lock-free work-stealing deque for thread pool
// Uses atomic operations for work-stealing without locks
// Optimized for cache efficiency and minimal contention

use std::sync::atomic::{AtomicPtr, AtomicUsize, AtomicBool, Ordering};
use std::ptr;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::sync::Arc;
use std::future::Future as StdFuture;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

struct QueueNode<T> {
    value: T,
    next: AtomicPtr<QueueNode<T>>,
    prev: AtomicPtr<QueueNode<T>>,
}

pub struct LockFreeDeque<T> {
    head: AtomicPtr<QueueNode<T>>,
    tail: AtomicPtr<QueueNode<T>>,
    _marker: std::marker::PhantomData<T>,
}

impl<T> LockFreeDeque<T> {
    pub fn new() -> Self {
        Self {
            head: AtomicPtr::new(ptr::null_mut()),
            tail: AtomicPtr::new(ptr::null_mut()),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn push_local(&self, value: T) {
        let node = Box::into_raw(Box::new(QueueNode {
            value,
            next: AtomicPtr::new(ptr::null_mut()),
            prev: AtomicPtr::new(ptr::null_mut()),
        }));

        loop {
            let old_tail = self.tail.load(Ordering::Acquire);
            if old_tail.is_null() {
                if self.tail.compare_exchange(old_tail, node, Ordering::Release, Ordering::Relaxed).is_ok() {
                    self.head.store(node, Ordering::Release);
                    return;
                }
            } else {
                unsafe {
                    (*old_tail).next.store(node, Ordering::Release);
                    (*node).prev.store(old_tail, Ordering::Release);
                }
                if self.tail.compare_exchange(old_tail, node, Ordering::Release, Ordering::Relaxed).is_ok() {
                    return;
                }
            }
        }
    }

    pub fn pop_local(&self) -> Option<T> {
        loop {
            let old_tail = self.tail.load(Ordering::Acquire);
            if old_tail.is_null() {
                return None;
            }

            let prev = unsafe { (*old_tail).prev.load(Ordering::Acquire) };
            
            if prev.is_null() {
                if self.tail.compare_exchange(old_tail, ptr::null_mut(), Ordering::Release, Ordering::Relaxed).is_ok() {
                    self.head.store(ptr::null_mut(), Ordering::Release);
                    let node = unsafe { Box::from_raw(old_tail) };
                    return Some(node.value);
                }
            } else {
                if self.tail.compare_exchange(old_tail, prev, Ordering::Release, Ordering::Relaxed).is_ok() {
                    unsafe {
                        (*prev).next.store(ptr::null_mut(), Ordering::Release);
                    }
                    let node = unsafe { Box::from_raw(old_tail) };
                    return Some(node.value);
                }
            }
        }
    }

    pub fn steal(&self) -> Option<T> {
        loop {
            let old_head = self.head.load(Ordering::Acquire);
            if old_head.is_null() {
                return None;
            }

            let next = unsafe { (*old_head).next.load(Ordering::Acquire) };
            
            if next.is_null() {
                if self.head.compare_exchange(old_head, ptr::null_mut(), Ordering::Release, Ordering::Relaxed).is_ok() {
                    let _ = self.tail.compare_exchange(old_head, ptr::null_mut(), Ordering::Release, Ordering::Relaxed);
                    let node = unsafe { Box::from_raw(old_head) };
                    return Some(node.value);
                }
            } else {
                if self.head.compare_exchange(old_head, next, Ordering::Release, Ordering::Relaxed).is_ok() {
                    unsafe {
                        (*next).prev.store(ptr::null_mut(), Ordering::Release);
                    }
                    let node = unsafe { Box::from_raw(old_head) };
                    return Some(node.value);
                }
            }
        }
    }
}

impl<T> Default for LockFreeDeque<T> {
    fn default() -> Self { Self::new() }
}

impl<T> Drop for LockFreeDeque<T> {
    fn drop(&mut self) {
        while self.pop_local().is_some() {}
    }
}