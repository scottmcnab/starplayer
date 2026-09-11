//! Flash storage: the WiFi credentials in the `config` partition, and stored module
//! images in the `modules` partition.
//!
//! # The hazard this whole module is shaped around
//!
//! On the classic ESP32 an erase or a write turns the **instruction and data caches off**
//! for its duration. Everything mapped through that cache — code executing from flash,
//! `include_bytes!` module images, and PSRAM, which is reached through the same cache —
//! is unreadable while it is off. esp-storage encodes the consequence in its own API:
//! a write refuses with `OtherCoreRunning` unless the second core is parked, because that
//! core is executing from flash and would fetch garbage.
//!
//! This firmware runs the audio refill on core 1 (M8-I5), so:
//!
//! * [`Store::take`] builds its `FlashStorage` with `multicore_auto_park()`. esp-storage
//!   parks core 1 around each erase/write chunk and unparks it afterwards. Core 1 is not
//!   *running* during a flash write, which means it is not refilling the DMA ring either.
//! * The caller therefore **pauses playback first** — `web.rs`'s `with_playback_paused`
//!   stops the transport, waits for the ramp and for the DMA ring to drain to silence,
//!   runs the write, and plays again. A hard write without that pause is a loud repeated
//!   fragment of whatever the ring happened to hold, not a gap.
//! * Reads need none of this. esp-storage's read path drives the SPI controller from
//!   IRAM without disabling the cache, and its own multi-core check is on the write path
//!   alone. A large read does contend for the flash bus and will cost underruns, so
//!   [`Store::read_slot`] is called under the same pause — for audio quality, not for
//!   safety.
//!
//! # The partition table is read, never hardcoded
//!
//! `partitions.csv` says where `config` and `modules` are, and the bootloader's copy of
//! that table at 0x8000 is the one the device actually booted with. Matching by **label**
//! rather than by subtype matters: `nvs`, `config` and `modules` are all data partitions
//! and two of them share a subtype.

use alloc::vec::Vec;
use core::ops::Range;

use embassy_embedded_hal::adapter::BlockingAsync;
use embedded_storage::{ReadStorage, Storage};
use esp_bootloader_esp_idf::partitions;
use esp_storage::FlashStorage;
use sequential_storage::cache::Cache;
use sequential_storage::map::{MapConfig, MapStorage};

/// The label of the partition the WiFi credentials live in, as `partitions.csv` spells it.
const CONFIG_PARTITION: &str = "config";
/// The label of the partition stored module images live in.
const MODULES_PARTITION: &str = "modules";

/// `sequential-storage` key for the WiFi credentials record. A `u8` keyspace, with room
/// for whatever a later milestone wants to keep beside them.
const KEY_WIFI: u8 = 1;

/// Longest SSID an IEEE 802.11 beacon can carry.
pub const SSID_MAX_BYTES: usize = 32;
/// Longest WPA2 passphrase. 63 is the ASCII maximum; 64 is the hexadecimal PSK form,
/// which `esp-radio` also accepts.
pub const PASSPHRASE_MAX_BYTES: usize = 64;

/// Scratch for one `sequential-storage` item: the key, the value and the flash-word
/// padding around them. The stored record is at most
/// `2 + SSID_MAX_BYTES + PASSPHRASE_MAX_BYTES` = 98 bytes.
const CONFIG_SCRATCH_BYTES: usize = 160;

/// How much flash one stored module gets.
///
/// 256 KiB, so the 1 408 KiB `modules` partition holds five of them with room to spare —
/// five is also the number the page's slot selector offers. The first 4 KiB (one erase
/// sector) is the slot header; the rest is the image.
pub const SLOT_BYTES: u32 = 256 * 1024;
/// The slot header's own erase sector. The image starts after it, which keeps the header
/// writable without touching the image and keeps the image 4-byte aligned for
/// [`Module::from_image`](starplayer_model::Module::from_image).
const SLOT_HEADER_BYTES: u32 = 4096;
/// The largest image a slot can hold.
pub const SLOT_IMAGE_MAX_BYTES: u32 = SLOT_BYTES - SLOT_HEADER_BYTES;
/// Slot-header magic: "StarPlayer Module Slot".
const SLOT_MAGIC: [u8; 4] = *b"SPMS";
/// Slot-header version. Bumped if the header layout below ever changes; an unknown
/// version reads as an empty slot rather than as a mis-parsed one.
const SLOT_VERSION: u16 = 1;
/// Bytes of the slot header actually used: magic, version, flags, length, name.
const SLOT_HEADER_USED: usize = 4 + 2 + 2 + 4 + SLOT_NAME_BYTES;
/// How much of a module's title a slot header keeps.
pub const SLOT_NAME_BYTES: usize = 16;

/// What a store operation could not do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// `partitions.csv`'s `config` or `modules` row was not in the table the device
    /// booted with. A firmware flashed with the wrong partition table.
    PartitionMissing(&'static str),
    /// The partition table at 0x8000 could not be read or did not parse.
    PartitionTable,
    /// The `config` partition is not a shape `sequential-storage` can use — not on an
    /// erase-sector boundary, or under two sectors long.
    ConfigRange,
    /// The flash refused a read or a write. On a write this is most often
    /// `OtherCoreRunning`, which means the pause described in the module docs was skipped.
    Flash,
    /// A stored record was there but did not decode.
    Corrupt,
    /// A slot number past [`Store::slot_count`].
    NoSuchSlot,
    /// The image is larger than [`SLOT_IMAGE_MAX_BYTES`].
    TooLarge,
    /// The slot holds no module.
    SlotEmpty,
}

/// What the captive portal collected and the station mode connects with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiCredentials {
    pub ssid: heapless::String<SSID_MAX_BYTES>,
    pub passphrase: heapless::String<PASSPHRASE_MAX_BYTES>,
}

impl WifiCredentials {
    /// Encode as `[ssid length][ssid][passphrase length][passphrase]`.
    ///
    /// Written by hand rather than through `serde`: the record is two strings, the format
    /// is read by exactly one function twenty lines below, and a derive would pull a
    /// serialiser into the firmware for 98 bytes of data.
    fn encode(&self, out: &mut Vec<u8>) {
        out.clear();
        out.push(self.ssid.len() as u8);
        out.extend_from_slice(self.ssid.as_bytes());
        out.push(self.passphrase.len() as u8);
        out.extend_from_slice(self.passphrase.as_bytes());
    }

    /// Decode what [`WifiCredentials::encode`] wrote. `None` for anything else, including
    /// a record from a future firmware with more fields.
    fn decode(bytes: &[u8]) -> Option<WifiCredentials> {
        let (&ssid_length, rest) = bytes.split_first()?;
        let (ssid, rest) = rest.split_at_checked(usize::from(ssid_length))?;
        let (&passphrase_length, rest) = rest.split_first()?;
        let (passphrase, _) = rest.split_at_checked(usize::from(passphrase_length))?;
        Some(WifiCredentials {
            ssid: heapless::String::try_from(core::str::from_utf8(ssid).ok()?).ok()?,
            passphrase: heapless::String::try_from(core::str::from_utf8(passphrase).ok()?).ok()?,
        })
    }
}

/// One stored module image's header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlotHeader {
    /// The module's title, as far as [`SLOT_NAME_BYTES`] allows.
    pub name: firmware_common::FixedStr<SLOT_NAME_BYTES>,
    /// The image's length in bytes.
    pub bytes: u32,
}

/// The device's flash, as the two partitions this firmware writes.
///
/// One instance, owned by the control task — the task that already owns
/// [`ControlHalf`](starplayer_host_embedded::ControlHalf) — so that "a single write is
/// ever in flight" needs no lock to be true.
pub struct Store {
    flash: FlashStorage<'static>,
    config: Range<u32>,
    modules: Range<u32>,
    /// Scratch for one `sequential-storage` item. A field rather than a local, because a
    /// 160-byte buffer on the caller's stack is 160 bytes of a main stack that already
    /// carries esp-storage's own 4 KiB sector buffer during the same call.
    scratch: [u8; CONFIG_SCRATCH_BYTES],
}

impl Store {
    /// Claim the flash and find the two partitions.
    ///
    /// Called once, from `main`, **before the second core is started**: reading the
    /// partition table is a flash read with a ~3 KiB stack frame, and doing it while the
    /// audio refill is running would cost underruns for no reason.
    pub fn take(flash: esp_hal::peripherals::FLASH<'static>) -> Result<Store, StoreError> {
        // `multicore_auto_park`: see the module docs. Without it every write on this board
        // fails with `OtherCoreRunning` the moment the audio refill is up.
        let mut flash = FlashStorage::new(flash).multicore_auto_park();

        let mut table_bytes = [0u8; partitions::PARTITION_TABLE_MAX_LEN];
        let table = partitions::read_partition_table(&mut flash, &mut table_bytes).map_err(|_| StoreError::PartitionTable)?;

        let mut config = None;
        let mut modules = None;
        for entry in table.iter() {
            let range = entry.offset()..entry.offset() + entry.len();
            match entry.label_as_str() {
                CONFIG_PARTITION => config = Some(range),
                MODULES_PARTITION => modules = Some(range),
                _ => {}
            }
        }
        let config = config.ok_or(StoreError::PartitionMissing(CONFIG_PARTITION))?;
        let modules = modules.ok_or(StoreError::PartitionMissing(MODULES_PARTITION))?;

        // Validated once, here, so that every later `MapConfig::new` — which panics on a
        // bad range — is provably infallible. A panic on a device is a reset.
        MapConfig::<BlockingAsync<&mut FlashStorage<'static>>>::try_new(config.clone()).map_err(|_| StoreError::ConfigRange)?;

        Ok(Store { flash, config, modules, scratch: [0; CONFIG_SCRATCH_BYTES] })
    }

    /// The stored WiFi credentials, or `None` when the device has never been provisioned.
    pub async fn load_wifi(&mut self) -> Result<Option<WifiCredentials>, StoreError> {
        let Store { flash, config, scratch, .. } = self;
        let mut map = MapStorage::new(BlockingAsync::new(flash), MapConfig::new(config.clone()), Cache::new_uncached());
        match map.fetch_item::<&[u8]>(scratch, &KEY_WIFI).await {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => match WifiCredentials::decode(bytes) {
                // An empty SSID is what `erase_wifi` writes, and what a device that has
                // never been provisioned looks like.
                Some(credentials) if credentials.ssid.is_empty() => Ok(None),
                Some(credentials) => Ok(Some(credentials)),
                None => Err(StoreError::Corrupt),
            },
            Err(_) => Err(StoreError::Flash),
        }
    }

    /// Store the credentials the captive portal collected. **A flash write** — pause
    /// playback around it.
    pub async fn save_wifi(&mut self, credentials: &WifiCredentials) -> Result<(), StoreError> {
        let mut encoded = Vec::with_capacity(2 + credentials.ssid.len() + credentials.passphrase.len());
        credentials.encode(&mut encoded);
        let Store { flash, config, scratch, .. } = self;
        let mut map = MapStorage::new(BlockingAsync::new(flash), MapConfig::new(config.clone()), Cache::new_uncached());
        map.store_item(scratch, &KEY_WIFI, &encoded.as_slice()).await.map_err(|_| StoreError::Flash)
    }

    /// Forget the credentials, so the next boot comes up as the captive portal. **A flash
    /// write** — pause playback around it.
    ///
    /// Written as an *empty* record rather than removed. `sequential-storage`'s
    /// `remove_all_items` needs `MultiwriteNorFlash`, which `esp-storage` implements for
    /// `FlashStorage` but not for the `&mut FlashStorage` this store hands it — and an
    /// empty SSID is already the "never provisioned" answer everywhere that reads it
    /// ([`Store::load_wifi`] returns `None` for one), so the removal buys nothing the
    /// write does not.
    pub async fn erase_wifi(&mut self) -> Result<(), StoreError> {
        self.save_wifi(&WifiCredentials { ssid: heapless::String::new(), passphrase: heapless::String::new() }).await
    }

    /// How many module slots the `modules` partition holds.
    pub fn slot_count(&self) -> u8 { ((self.modules.end - self.modules.start) / SLOT_BYTES).min(u32::from(u8::MAX)) as u8 }

    /// Where slot `id` (one-based, as the API numbers them) starts.
    fn slot_offset(&self, id: u8) -> Result<u32, StoreError> {
        if id == 0 || id > self.slot_count() {
            return Err(StoreError::NoSuchSlot);
        }
        Ok(self.modules.start + u32::from(id - 1) * SLOT_BYTES)
    }

    /// Read slot `id`'s header, or `None` when the slot is empty or holds something this
    /// firmware does not recognise.
    pub fn slot_header(&mut self, id: u8) -> Result<Option<SlotHeader>, StoreError> {
        let offset = self.slot_offset(id)?;
        let mut header = [0u8; SLOT_HEADER_USED];
        self.flash.read(offset, &mut header).map_err(|_| StoreError::Flash)?;
        if header[..4] != SLOT_MAGIC {
            return Ok(None);
        }
        if u16::from_le_bytes([header[4], header[5]]) != SLOT_VERSION {
            return Ok(None);
        }
        let bytes = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        if bytes == 0 || bytes > SLOT_IMAGE_MAX_BYTES {
            return Ok(None);
        }
        let name = &header[12..12 + SLOT_NAME_BYTES];
        let name_length = name.iter().position(|byte| *byte == 0).unwrap_or(SLOT_NAME_BYTES);
        let name = firmware_common::FixedStr::new(core::str::from_utf8(&name[..name_length]).unwrap_or(""));
        Ok(Some(SlotHeader { name, bytes }))
    }

    /// Read slot `id`'s image into `destination`, returning how many bytes it holds.
    ///
    /// A read, not a write, so it cannot corrupt anything — but it is a long one (a whole
    /// module image through esp-storage's 4 KiB sector buffer) and contends for the flash
    /// bus with the audio refill's own instruction fetches, so the caller pauses playback
    /// around it all the same.
    pub fn read_slot(&mut self, id: u8, destination: &mut [u8]) -> Result<usize, StoreError> {
        let offset = self.slot_offset(id)?;
        let header = self.slot_header(id)?.ok_or(StoreError::SlotEmpty)?;
        let bytes = header.bytes as usize;
        let destination = destination.get_mut(..bytes).ok_or(StoreError::TooLarge)?;
        self.flash.read(offset + SLOT_HEADER_BYTES, destination).map_err(|_| StoreError::Flash)?;
        Ok(bytes)
    }

    /// Write `image` into slot `id` under `name`. **A long flash write** — several
    /// seconds for a 90 KB image, with the cache off in bursts and core 1 parked for each
    /// of them. Pause playback around it.
    ///
    /// The header is written **last**, and zeroed first, so that a power loss part-way
    /// through leaves an empty slot rather than a header pointing at half an image.
    pub fn write_slot(&mut self, id: u8, name: &str, image: &[u8]) -> Result<(), StoreError> {
        let offset = self.slot_offset(id)?;
        if image.len() as u32 > SLOT_IMAGE_MAX_BYTES {
            return Err(StoreError::TooLarge);
        }

        let blank = [0u8; SLOT_HEADER_USED];
        self.flash.write(offset, &blank).map_err(|_| StoreError::Flash)?;
        self.flash.write(offset + SLOT_HEADER_BYTES, image).map_err(|_| StoreError::Flash)?;

        let mut header = [0u8; SLOT_HEADER_USED];
        header[..4].copy_from_slice(&SLOT_MAGIC);
        header[4..6].copy_from_slice(&SLOT_VERSION.to_le_bytes());
        header[8..12].copy_from_slice(&(image.len() as u32).to_le_bytes());
        let name = firmware_common::FixedStr::<SLOT_NAME_BYTES>::new(name);
        let name = name.as_str().as_bytes();
        header[12..12 + name.len()].copy_from_slice(name);
        self.flash.write(offset, &header).map_err(|_| StoreError::Flash)
    }
}
