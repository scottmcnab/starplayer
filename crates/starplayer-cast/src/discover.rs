//! Finding Cast devices on the local network with mDNS.
//!
//! Cast devices answer `_googlecast._tcp.local.` and put everything a picker needs in the
//! TXT record: `fn` is the friendly name the owner gave the device, `md` is the model
//! ("Google Nest Mini", "Google Cast Group"), and `id` is a stable identifier that
//! survives a rename.
//!
//! **A speaker group announces itself exactly like a single device** — same service type,
//! same TXT keys, same CASTv2 endpoint — so nothing here filters one out. `md` is how the
//! listing tells the reader which is which, and casting to a group is how multi-room
//! works.
//!
//! Finding nothing is a normal result, not an error. mDNS is multicast, and multicast does
//! not cross a NAT: inside WSL2's default networking, inside most containers, and across a
//! VLAN boundary, a browse that works perfectly will still return an empty list. The
//! caller is expected to say so rather than to fail.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent};

use crate::{CAST_PORT, CAST_SERVICE_TYPE, CastError};

/// One Cast device, or one speaker group, as a picker sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CastDevice {
    /// The name the owner gave it, from the TXT `fn` key. Falls back to the mDNS service
    /// instance name when the record has no `fn`.
    pub friendly_name: String,
    /// The TXT `md` key: "Google Nest Mini", "Chromecast", "Google Cast Group", …
    pub model: String,
    /// The TXT `id` key — stable across renames, and what this listing de-duplicates on.
    pub id: String,
    /// The address to open a CASTv2 connection to.
    pub address: IpAddr,
    /// The CASTv2 port, almost always [`CAST_PORT`].
    pub port: u16,
}

impl CastDevice {
    /// One line for `--list`: name, model, and where to reach it.
    pub fn describe(&self) -> String {
        format!("{}  [{}]  {}:{}", self.friendly_name, self.model, self.address, self.port)
    }
}

/// Browse `_googlecast._tcp.local.` for `timeout`, and return what answered.
///
/// The whole timeout is always spent: mDNS has no "that is everyone" signal, so the only
/// way to be reasonably sure a slow device was heard is to keep listening. Responses are
/// de-duplicated on the TXT `id` — a device with both an IPv4 and an IPv6 address answers
/// more than once — and the result is sorted by `friendly_name` so two runs on an
/// unchanged network print the same list in the same order.
pub fn discover(timeout: Duration) -> Result<Vec<CastDevice>, CastError> {
    let daemon = ServiceDaemon::new().map_err(|error| CastError::Discovery(format!("could not start the mDNS responder: {error}")))?;
    let receiver = daemon
        .browse(CAST_SERVICE_TYPE)
        .map_err(|error| CastError::Discovery(format!("could not browse {CAST_SERVICE_TYPE}: {error}")))?;

    let deadline = Instant::now() + timeout;
    // Keyed by TXT `id`, so the map itself is the de-duplication and the last (most
    // complete) answer for a device wins.
    let mut found: BTreeMap<String, CastDevice> = BTreeMap::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(resolved)) => {
                if let Some(device) = device_from_resolved(&resolved) {
                    found.insert(device.id.clone(), device);
                }
            }
            Ok(_) => {}
            // The daemon dropped the browse, or the deadline arrived. Either way there is
            // nothing more to wait for.
            Err(_) => break,
        }
    }
    // Shutting the daemon down is best-effort: it returns a channel reporting when its
    // thread has finished, and a failure here changes nothing about what was found.
    let _ = daemon.shutdown();

    let mut devices: Vec<CastDevice> = found.into_values().collect();
    devices.sort_by(|left, right| {
        left.friendly_name
            .to_lowercase()
            .cmp(&right.friendly_name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(devices)
}

/// Turn one resolved mDNS service into a [`CastDevice`], or `None` if it has no address.
fn device_from_resolved(resolved: &mdns_sd::ResolvedService) -> Option<CastDevice> {
    let address = resolved.get_addresses().iter().map(|scoped| scoped.to_ip_addr()).min_by_key(|address| match address {
        // A speaker reachable both ways is reached over IPv4: that is the address the
        // media server will bind a matching interface for, and the one a receiver
        // fetching an `http://` URL handles without a bracketed literal.
        IpAddr::V4(_) => 0u8,
        IpAddr::V6(_) => 1u8,
    })?;

    // `fn` is the only field a device could plausibly omit, and dropping a device because
    // its TXT record is thin would hide it from the picker entirely. The instance name —
    // the first label of the fullname — is what the Google Home app falls back to too.
    let friendly_name = resolved
        .get_property_val_str("fn")
        .map(str::to_string)
        .unwrap_or_else(|| instance_name(resolved.get_fullname()));
    let model = resolved.get_property_val_str("md").unwrap_or("unknown model").to_string();
    let id = resolved.get_property_val_str("id").map(str::to_string).unwrap_or_else(|| resolved.get_fullname().to_string());
    let port = if resolved.get_port() == 0 { CAST_PORT } else { resolved.get_port() };

    Some(CastDevice { friendly_name, model, id, address, port })
}

/// The instance label of an mDNS fullname — everything before the service type.
fn instance_name(fullname: &str) -> String {
    fullname.split_once("._googlecast").map(|(instance, _)| instance.to_string()).unwrap_or_else(|| fullname.to_string())
}

/// Find the one device whose friendly name starts with `name`, case-insensitively.
///
/// A prefix rather than an exact match, so `--device kit` finds "Kitchen speaker" without
/// anyone having to type the name the way the Google Home app capitalised it. An ambiguous
/// prefix is an error that names every match: silently taking the first would send music
/// to the wrong room, which is a worse outcome than an error message.
pub fn find<'a>(devices: &'a [CastDevice], name: &str) -> Result<&'a CastDevice, CastError> {
    let wanted = name.trim().to_lowercase();
    let matches: Vec<&CastDevice> = devices.iter().filter(|device| device.friendly_name.to_lowercase().starts_with(&wanted)).collect();
    match matches.as_slice() {
        [] => Err(CastError::Discovery(format!("no cast device's name starts with {name:?}; `starplayer cast --list` shows what answered"))),
        [only] => Ok(only),
        many => {
            let names: Vec<&str> = many.iter().map(|device| device.friendly_name.as_str()).collect();
            Err(CastError::Discovery(format!("{name:?} matches more than one device: {}", names.join(", "))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn device(friendly_name: &str, model: &str, id: &str) -> CastDevice {
        CastDevice {
            friendly_name: String::from(friendly_name),
            model: String::from(model),
            id: String::from(id),
            address: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40)),
            port: CAST_PORT,
        }
    }

    #[test]
    fn a_prefix_finds_one_device_whatever_its_case() {
        let devices = [device("Kitchen speaker", "Google Nest Mini", "a"), device("Study", "Chromecast", "b")];
        assert_eq!(find(&devices, "kit").unwrap().id, "a");
        assert_eq!(find(&devices, "STUD").unwrap().id, "b");
        assert_eq!(find(&devices, "Kitchen speaker").unwrap().id, "a");
    }

    #[test]
    fn an_ambiguous_prefix_names_every_match_rather_than_guessing() {
        let devices = [device("Kitchen speaker", "Google Nest Mini", "a"), device("Kitchen group", "Google Cast Group", "b")];
        let error = find(&devices, "kitchen").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Kitchen speaker") && message.contains("Kitchen group"), "{message}");
    }

    #[test]
    fn a_prefix_that_matches_nothing_points_at_the_listing() {
        let devices = [device("Kitchen speaker", "Google Nest Mini", "a")];
        let message = find(&devices, "bedroom").unwrap_err().to_string();
        assert!(message.contains("--list"), "{message}");
    }

    #[test]
    fn a_group_is_listed_like_any_other_device() {
        let group = device("Whole house", "Google Cast Group", "g");
        assert!(group.describe().contains("Google Cast Group"), "the model is what tells a group apart");
    }

    #[test]
    fn an_instance_name_is_the_label_before_the_service_type() {
        assert_eq!(instance_name("Chromecast-abc._googlecast._tcp.local."), "Chromecast-abc");
        assert_eq!(instance_name("odd"), "odd");
    }
}
