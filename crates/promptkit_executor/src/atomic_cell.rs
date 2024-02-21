#![allow(dead_code)]

use std::{
    marker::PhantomData,
    ptr::null_mut,
    sync::atomic::{AtomicPtr, Ordering},
};

pub struct AtomicCell<T> {
    ptr: AtomicPtr<T>,
    phantom: PhantomData<Box<T>>,
}

impl<T> AtomicCell<T> {
    pub fn new(t: T) -> Self {
        Self {
            ptr: AtomicPtr::new(Box::into_raw(Box::new(t))),
            phantom: PhantomData,
        }
    }

    pub const fn empty() -> Self {
        Self {
            ptr: AtomicPtr::new(null_mut()),
            phantom: PhantomData,
        }
    }

    pub fn set(&mut self, t: Option<T>) {
        let ptr = t.map_or(null_mut(), |t| Box::into_raw(Box::new(t)));
        let old = self.ptr.swap(ptr, Ordering::Release);
        if !old.is_null() {
            drop(unsafe { Box::from_raw(old) });
        }
    }

    // Must ensure that there is no concurrent access (with, with_async) to the cell
    pub unsafe fn set_unguarded(&self, t: Option<T>) {
        let ptr = t.map_or(null_mut(), |t| Box::into_raw(Box::new(t)));
        let old = self.ptr.swap(ptr, Ordering::Release);
        if !old.is_null() {
            drop(unsafe { Box::from_raw(old) });
        }
    }

    pub fn is_null(&self) -> bool {
        self.ptr.load(Ordering::Relaxed).is_null()
    }
}

impl<T> Drop for AtomicCell<T> {
    fn drop(&mut self) {
        self.set(None);
    }
}

impl<T> AtomicCell<T> {
    pub fn with_mut<O>(&mut self, f: impl FnOnce(&mut T) -> O) -> Option<O> {
        let ptr = self.ptr.load(Ordering::Acquire);
        unsafe { ptr.as_mut() }.map(f)
    }

    pub async fn with_mut_async<'s, 'o, O, F>(
        &'s mut self,
        f: impl FnOnce(&'o mut T) -> F + 'o,
    ) -> Option<O>
    where
        F: std::future::Future<Output = O> + 'o,
        's: 'o,
    {
        let ptr = self.ptr.load(Ordering::Acquire);
        if let Some(t) = unsafe { ptr.as_mut() } {
            Some(f(t).await)
        } else {
            None
        }
    }
}

impl<T: Sync + Send> AtomicCell<T> {
    pub fn with<O>(&self, f: impl FnOnce(&T) -> O) -> Option<O> {
        let ptr = self.ptr.load(Ordering::Acquire);
        unsafe { ptr.as_ref() }.map(f)
    }

    pub async fn with_async<'s, 'o, O, F>(&'s self, f: impl FnOnce(&'o T) -> F + 'o) -> Option<O>
    where
        F: std::future::Future<Output = O> + 'o,
        's: 'o,
    {
        let ptr = self.ptr.load(Ordering::Acquire);
        if let Some(t) = unsafe { ptr.as_ref() } {
            Some(f(t).await)
        } else {
            None
        }
    }
}

impl<T> Default for AtomicCell<T> {
    fn default() -> Self {
        Self::empty()
    }
}
