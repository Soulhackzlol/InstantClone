//! Blocking `Mutex` / `RwLock` with the poison `unwrap` in one place.
//!
//! InstantClone ships with `panic = "abort"` (see `[profile.release]` in
//! Cargo.toml), so in the release binary a lock can never be observed
//! poisoned: a panic tears the whole process down instead of unwinding out of
//! a held guard, so no other thread ever sees a half-updated "poisoned" lock.
//! That makes the `LockResult` on `std`'s locks pure noise at every call site.
//!
//! These thin newtypes move that `unwrap` into a single place. `lock` / `read`
//! / `write` hand back the guard directly, so call sites read as `.lock()`
//! instead of `.lock().unwrap()`, and the "this cannot actually fail" reasoning
//! lives here once rather than being re-derived at ~80 sites. Behavior is
//! identical to the old per-site `.unwrap()`: infallible in release, and a loud
//! fail-fast panic on the unwind-only poisoned path in debug/test builds (the
//! right signal - a poisoned lock means a thread already panicked mid-mutation,
//! so recovering would just mask it). We deliberately do *not* recover via
//! `into_inner`: that pattern exists to keep an unwinding long-running server
//! alive across a panicking request, which does not apply when every panic
//! aborts the process anyway.
//!
//! Same memory layout and codegen as the `std` locks, no dependencies, and new
//! code is clean by construction rather than by an `.unwrap()` convention
//! everyone has to remember.

use std::sync::{MutexGuard, RwLockReadGuard, RwLockWriteGuard};

/// A `std::sync::Mutex` whose `lock` returns the guard directly.
#[derive(Debug, Default)]
pub struct Mutex<T>(std::sync::Mutex<T>);

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }

    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}

/// A `std::sync::RwLock` whose `read` / `write` return the guard directly.
#[derive(Debug, Default)]
pub struct RwLock<T>(std::sync::RwLock<T>);

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self(std::sync::RwLock::new(value))
    }

    #[inline]
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.0.read().unwrap()
    }

    #[inline]
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        self.0.write().unwrap()
    }
}
