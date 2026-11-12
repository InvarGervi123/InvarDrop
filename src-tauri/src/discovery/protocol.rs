use super::Device;
use mdns_sd::{ResolvedService, ServiceInfo};
use uuid::Uuid;

pub(super) const SERVICE_TYPE: &str = "_invardrop._tcp.local.";

pub(super) fn local_device() -> Device {
    let id = Uuid::new_v4().to_string();
    let name = format!("InvarDrop {}", &id[..8]);
    Device { id, name }
}

pub(super) fn announcement(local: &Device, heartbeat: u64) -> mdns_sd::Result<ServiceInfo> {
    let marker = heartbeat.to_string();
    let props = [
        ("id", local.id.as_str()),
        ("name", local.name.as_str()),
        ("v", "1"),
        ("mode", "discovery"),
        ("beat", marker.as_str()),
    ];
    // Port zero is intentional: no transfer endpoint exists in this milestone.
    let mut info = ServiceInfo::new(
        SERVICE_TYPE,
        &local.id,
        &format!("invardrop-{}.local.", local.id),
        "",
        0,
        &props[..],
    )?
    .enable_addr_auto();
    info.set_requires_probe(heartbeat == 0);
    Ok(info)
}

pub(super) fn parse(service: &ResolvedService) -> Option<(Device, u64)> {
    if service.fullname.len() > 255
        || !service
            .fullname
            .to_ascii_lowercase()
            .ends_with(SERVICE_TYPE)
        || !service.is_valid()
        || !service.ty_domain.eq_ignore_ascii_case(SERVICE_TYPE)
        || service.get_property_val_str("v") != Some("1")
        || service.get_property_val_str("mode") != Some("discovery")
    {
        return None;
    }
    let id = Uuid::parse_str(service.get_property_val_str("id")?).ok()?;
    if id.get_version_num() != 4 || id.get_variant() != uuid::Variant::RFC4122 {
        return None;
    }
    let name = service.get_property_val_str("name")?.trim();
    if name.is_empty() || name.len() > 80 || name.chars().any(|c|
        c.is_control() || matches!(c, '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
        return None;
    }
    let heartbeat = service.get_property_val_str("beat")?.parse().ok()?;
    Some((
        Device {
            id: id.to_string(),
            name: name.into(),
        },
        heartbeat,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resolved(props: &[(&str, &str)]) -> ResolvedService {
        ServiceInfo::new(
            SERVICE_TYPE,
            "test",
            "random.local.",
            "192.168.1.2",
            0,
            props,
        )
        .unwrap()
        .as_resolved_service()
    }
    #[test]
    fn announcement_is_anonymous_unique_and_has_no_transfer_endpoint() {
        let a = local_device();
        let b = local_device();
        assert_ne!(a.id, b.id);
        assert!(a.name.starts_with("InvarDrop "));
        let info = announcement(&a, 12).unwrap();
        assert_eq!(info.get_port(), 0);
        assert!(info.is_addr_auto());
        assert_eq!(info.get_properties().len(), 5);
        assert_eq!(info.get_hostname(), format!("invardrop-{}.local.", a.id));
    }
    #[test]
    fn validates_protocol_identity_and_untrusted_names() {
        let id = "C015841B-F164-4D98-B880-D9F0E1952BB4";
        let mut props = [
            ("id", id),
            ("name", "Living room"),
            ("v", "1"),
            ("mode", "discovery"),
            ("beat", "2"),
        ];
        let (d, beat) = parse(&resolved(&props)).unwrap();
        assert_eq!(d.id, id.to_lowercase());
        assert_eq!(beat, 2);
        props[1].1 = "\u{202e}spoof";
        assert!(parse(&resolved(&props)).is_none());
        props[1].1 = "Valid";
        props[2].1 = "2";
        assert!(parse(&resolved(&props)).is_none());
        props[2].1 = "1";
        props[0].1 = "not-a-uuid";
        assert!(parse(&resolved(&props)).is_none());
        props[0].1 = id;
        props[4].1 = "invalid";
        assert!(parse(&resolved(&props)).is_none());
    }
    #[test]
    fn rejects_missing_metadata_and_oversized_names() {
        assert!(parse(&resolved(&[])).is_none());
        let long = "x".repeat(81);
        let props = [
            ("id", "c015841b-f164-4d98-b880-d9f0e1952bb4"),
            ("name", long.as_str()),
            ("v", "1"),
            ("mode", "discovery"),
            ("beat", "0"),
        ];
        assert!(parse(&resolved(&props)).is_none());
    }
}
