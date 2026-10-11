//! Helpers shared by the unit tests of several modules: a seeded random
//! source for the fuzz tests, and a scratch folder that cleans up after
//! itself even when the test fails.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Deterministic LCG for the fuzz tests: fixed seed, no dependency, and a
/// failure reproduces from the iteration number alone.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }

    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u32) as usize]
    }

    /// Zero- and 0xFF-heavy bytes. Uniform bytes almost never form a zero
    /// length prefix or a long Exp-Golomb run, which are exactly the shapes
    /// that break parsers.
    pub fn byte(&mut self) -> u8 {
        match self.below(8) {
            0..=2 => 0x00,
            3 => 0xFF,
            _ => self.next_u32() as u8,
        }
    }

    pub fn bytes(&mut self, max_len: u32) -> Vec<u8> {
        let n = self.below(max_len + 1);
        (0..n).map(|_| self.byte()).collect()
    }
}

/// A fresh, empty folder under the system temp dir, unique to this test
/// process and call, removed when dropped: a failing test leaves nothing
/// behind, and nothing a test writes can reach the user's real files.
pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ic-test-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create a test folder");
        TestDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_dir_is_empty_unique_and_gone_after_use() {
        let a = TestDir::new("self");
        let b = TestDir::new("self");
        assert_ne!(a.path(), b.path());
        std::fs::write(a.join("x"), b"x").unwrap();
        let kept = a.path().to_path_buf();
        drop(a);
        assert!(!kept.exists());
    }

    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        let (mut a, mut b) = (Lcg(7), Lcg(7));
        let first: Vec<u32> = (0..4).map(|_| a.next_u32()).collect();
        assert_eq!(first, (0..4).map(|_| b.next_u32()).collect::<Vec<_>>());
        assert!(first.windows(2).all(|w| w[0] != w[1]));
    }
}
