use super::Device;
use mdns_sd::{ResolvedService, ServiceInfo};
use uuid::Uuid;

pub(super) const SERVICE_TYPE: &str = "_invardrop._tcp.local.";

pub(super) fn local_device() -> Device {
    let id = Uuid::new_v4().to_string();
    let name = format!("InvarDrop {}", &id[..8]);
    Device {
        id,
        name,
        ..Default::default()
    }
}

pub(super) fn announcement(
    local: &Device,
    heartbeat: u64,
    port: u16,
) -> mdns_sd::Result<ServiceInfo> {
    let marker = heartbeat.to_string();
    let props = [
        ("id", local.id.as_str()),
        ("name", local.name.as_str()),
        ("v", if port == 0 { "1" } else { "2" }),
        ("mode", if port == 0 { "discovery" } else { "secure" }),
        ("beat", marker.as_str()),
    ];
    // Port zero is intentional: no transfer endpoint exists in this milestone.
    let mut info = ServiceInfo::new(
        SERVICE_TYPE,
        &local.id,
        &format!("invardrop-{}.local.", local.id),
        "",
        port,
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
        || !matches!(
            (
                service.get_property_val_str("v"),
                service.get_property_val_str("mode"),
                service.get_port()
            ),
            (Some("1"), Some("discovery"), 0) | (Some("2"), Some("secure"), 1..=u16::MAX)
        )
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
            can_connect: service.get_port() != 0 && !endpoints(service).is_empty(),
            endpoints: endpoints(service),
        },
        heartbeat,
    ))
}

fn endpoints(service: &ResolvedService) -> Vec<std::net::SocketAddr> {
    use mdns_sd::ScopedIp;
    use std::net::{SocketAddr, SocketAddrV6};
    if service.get_port() == 0 {
        return vec![];
    }
    let mut addresses = service
        .get_addresses()
        .iter()
        .filter_map(|ip| match ip {
            ScopedIp::V4(a)
                if a.addr().is_private() || a.addr().is_link_local() || a.addr().is_loopback() =>
            {
                Some(SocketAddr::new((*a.addr()).into(), service.get_port()))
            }
            ScopedIp::V6(a)
                if a.addr().is_unique_local()
                    || a.addr().is_unicast_link_local()
                    || a.addr().is_loopback() =>
            {
                Some(SocketAddr::V6(SocketAddrV6::new(
                    *a.addr(),
                    service.get_port(),
                    0,
                    a.scope_id().index,
                )))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    addresses.sort_by_key(|a| (!a.is_ipv4(), *a));
    addresses.dedup();
    addresses.truncate(8);
    addresses
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
        let info = announcement(&a, 12, 0).unwrap();
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
    #[test]
    fn secure_endpoints_are_versioned_and_public_addresses_are_not_dialed() {
        let local = local_device();
        let info = announcement(&local, 1, 30000).unwrap();
        assert!(info.get_property_val_str("mode") == Some("secure"));
        let props = [
            ("id", local.id.as_str()),
            ("name", local.name.as_str()),
            ("v", "2"),
            ("mode", "secure"),
            ("beat", "1"),
        ];
        let make = |ip| {
            ServiceInfo::new(SERVICE_TYPE, "test", "random.local.", ip, 30000, &props[..])
                .unwrap()
                .as_resolved_service()
        };
        let (local, _) = parse(&make("192.168.1.2")).unwrap();
        assert!(local.can_connect && local.endpoints.len() == 1);
        let (public, _) = parse(&make("8.8.8.8")).unwrap();
        assert!(!public.can_connect && public.endpoints.is_empty());
    }
}
