//! [`PcmStorage`] and [`BlobStorage`] — a [`Module`](crate::Module)'s two blobs, either
//! owned on the heap or borrowed from a memory-mapped module image.
//!
//! # Why `'static` and not a lifetime parameter
//!
//! A borrowed module's data comes from flash: an image linked into the firmware with
//! `include_bytes!`, or a flash partition mapped for the life of the program. That memory
//! outlives everything, so `'static` is not a restriction — it is the truth.
//!
//! The alternative, `Module<'image>`, would put a lifetime on `Module`, on
//! `Arc<Module>`, on `Engine<.., Arc<Module>>` and on every host type that names one, to
//! describe a borrow that in practice is always from a region that never goes away. That
//! is a large, permanent cost on every target to serve one, so the borrow is `'static`
//! and a caller that has no `'static` bytes uses
//! [`Module::from_image_copied`](crate::Module::from_image_copied) instead.
//!
//! # Equality, hashing and cloning
//!
//! All three are defined over the **slice contents**, never over which variant holds
//! them, so a borrowed module and the owned module it was built from compare equal and
//! hash alike. That is what keeps M10-K5a's identity-enhancer equality test and the
//! goldens' module fingerprinting true across the storage split.
//!
//! [`Clone`] is the one place the variant survives: cloning a borrowed blob produces
//! another borrow of the same bytes rather than a copy, because the whole point on an
//! embedded target is that those bytes are never copied into RAM.

use alloc::boxed::Box;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::ops::Deref;

/// A module's decoded `i16` sample frames: owned on the heap, or borrowed from a module
/// image in flash.
///
/// See the [module documentation](self) for why the borrow is `'static`.
pub enum PcmStorage {
    /// Built by [`ModuleBuilder`](crate::ModuleBuilder), and what every loader produces.
    Owned(Box<[i16]>),
    /// Borrowed in place from a memory-mapped module image
    /// ([`Module::from_image`](crate::Module::from_image)).
    Borrowed(&'static [i16]),
}

impl PcmStorage {
    /// The frames, whichever way they are held.
    pub const fn as_slice(&self) -> &[i16] {
        match self {
            PcmStorage::Owned(owned) => owned,
            PcmStorage::Borrowed(borrowed) => borrowed,
        }
    }

    /// Whether these frames are borrowed rather than owned — what a firmware's boot log
    /// prints to prove the PCM never entered RAM.
    pub const fn is_borrowed(&self) -> bool { matches!(self, PcmStorage::Borrowed(_)) }
}

impl Deref for PcmStorage {
    type Target = [i16];
    fn deref(&self) -> &[i16] { self.as_slice() }
}

/// Cloning a borrowed blob **stays borrowed**: the bytes are in flash and copying them
/// into RAM is exactly what this type exists to avoid.
impl Clone for PcmStorage {
    fn clone(&self) -> PcmStorage {
        match self {
            PcmStorage::Owned(owned) => PcmStorage::Owned(owned.clone()),
            PcmStorage::Borrowed(borrowed) => PcmStorage::Borrowed(borrowed),
        }
    }
}

/// Over the contents, so `Module`'s derived `Debug` prints the same text whichever
/// storage holds the frames.
impl fmt::Debug for PcmStorage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result { self.as_slice().fmt(formatter) }
}

impl PartialEq for PcmStorage {
    fn eq(&self, other: &PcmStorage) -> bool { self.as_slice() == other.as_slice() }
}

impl Eq for PcmStorage {}

impl Hash for PcmStorage {
    fn hash<H: Hasher>(&self, hasher: &mut H) { self.as_slice().hash(hasher); }
}

impl From<Box<[i16]>> for PcmStorage {
    fn from(owned: Box<[i16]>) -> PcmStorage { PcmStorage::Owned(owned) }
}

impl From<&'static [i16]> for PcmStorage {
    fn from(borrowed: &'static [i16]) -> PcmStorage { PcmStorage::Borrowed(borrowed) }
}

/// A module's concatenated **native** pattern bytes: owned on the heap, or borrowed from
/// a module image in flash. The `u8` counterpart of [`PcmStorage`].
pub enum BlobStorage {
    /// Built by [`ModuleBuilder`](crate::ModuleBuilder), and what every loader produces.
    Owned(Box<[u8]>),
    /// Borrowed in place from a memory-mapped module image
    /// ([`Module::from_image`](crate::Module::from_image)).
    Borrowed(&'static [u8]),
}

impl BlobStorage {
    /// The bytes, whichever way they are held.
    pub const fn as_slice(&self) -> &[u8] {
        match self {
            BlobStorage::Owned(owned) => owned,
            BlobStorage::Borrowed(borrowed) => borrowed,
        }
    }

    /// Whether these bytes are borrowed rather than owned.
    pub const fn is_borrowed(&self) -> bool { matches!(self, BlobStorage::Borrowed(_)) }
}

impl Deref for BlobStorage {
    type Target = [u8];
    fn deref(&self) -> &[u8] { self.as_slice() }
}

/// Cloning a borrowed blob stays borrowed; see [`PcmStorage::clone`].
impl Clone for BlobStorage {
    fn clone(&self) -> BlobStorage {
        match self {
            BlobStorage::Owned(owned) => BlobStorage::Owned(owned.clone()),
            BlobStorage::Borrowed(borrowed) => BlobStorage::Borrowed(borrowed),
        }
    }
}

impl fmt::Debug for BlobStorage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result { self.as_slice().fmt(formatter) }
}

impl PartialEq for BlobStorage {
    fn eq(&self, other: &BlobStorage) -> bool { self.as_slice() == other.as_slice() }
}

impl Eq for BlobStorage {}

impl Hash for BlobStorage {
    fn hash<H: Hasher>(&self, hasher: &mut H) { self.as_slice().hash(hasher); }
}

impl From<Box<[u8]>> for BlobStorage {
    fn from(owned: Box<[u8]>) -> BlobStorage { BlobStorage::Owned(owned) }
}

impl From<&'static [u8]> for BlobStorage {
    fn from(borrowed: &'static [u8]) -> BlobStorage { BlobStorage::Borrowed(borrowed) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// FNV-1a, for the same reason `module::tests` has one: `DefaultHasher` does not
    /// exist in a `no_std` crate.
    #[derive(Default)]
    struct TestHasher {
        state: u64,
    }

    impl Hasher for TestHasher {
        fn finish(&self) -> u64 { self.state }

        fn write(&mut self, bytes: &[u8]) {
            let mut state = match self.state {
                0 => 0xcbf2_9ce4_8422_2325,
                seeded => seeded,
            };
            for byte in bytes {
                state ^= *byte as u64;
                state = state.wrapping_mul(0x0000_0100_0000_01b3);
            }
            self.state = state;
        }
    }

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = TestHasher::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    static BORROWED_FRAMES: &[i16] = &[1, -2, 3];
    static BORROWED_BYTES: &[u8] = &[9, 8, 7];

    #[test]
    fn owned_and_borrowed_pcm_with_the_same_frames_are_equal_and_hash_alike() {
        let owned = PcmStorage::Owned(vec![1i16, -2, 3].into_boxed_slice());
        let borrowed = PcmStorage::Borrowed(BORROWED_FRAMES);

        assert_eq!(owned, borrowed, "equality is over the frames, not over the variant");
        assert_eq!(hash_of(&owned), hash_of(&borrowed));
        assert_eq!(owned.as_slice(), &[1i16, -2, 3]);
        assert_ne!(owned, PcmStorage::Owned(vec![1i16, -2, 4].into_boxed_slice()));
    }

    #[test]
    fn owned_and_borrowed_blobs_with_the_same_bytes_are_equal_and_hash_alike() {
        let owned = BlobStorage::Owned(vec![9u8, 8, 7].into_boxed_slice());
        let borrowed = BlobStorage::Borrowed(BORROWED_BYTES);

        assert_eq!(owned, borrowed);
        assert_eq!(hash_of(&owned), hash_of(&borrowed));
        assert_ne!(owned, BlobStorage::Owned(vec![9u8, 8, 6].into_boxed_slice()));
    }

    #[test]
    fn cloning_a_borrowed_blob_stays_borrowed() {
        let pcm = PcmStorage::Borrowed(BORROWED_FRAMES).clone();
        let blob = BlobStorage::Borrowed(BORROWED_BYTES).clone();

        assert!(pcm.is_borrowed(), "a clone of a borrowed module must not copy flash into RAM");
        assert!(blob.is_borrowed());
        assert!(!PcmStorage::Owned(vec![0i16].into_boxed_slice()).is_borrowed());
        assert!(!BlobStorage::Owned(vec![0u8].into_boxed_slice()).is_borrowed());
    }

    #[test]
    fn a_storage_derefs_to_its_slice() {
        let pcm = PcmStorage::Owned(vec![4i16, 5].into_boxed_slice());
        let blob = BlobStorage::Borrowed(BORROWED_BYTES);

        assert_eq!(pcm.len(), 2);
        assert_eq!(blob.first(), Some(&9u8));
    }
}
