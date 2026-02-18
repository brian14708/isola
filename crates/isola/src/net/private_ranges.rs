use std::net::IpAddr;

pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            // Catch IPv4-mapped IPv6 addresses (::ffff:x.x.x.x) that would
            // otherwise bypass private-range checks.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            v6.is_unique_local()
                || v6.is_loopback()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
                || v6.is_unspecified()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn ipv4_private() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    }

    #[test]
    fn ipv4_mapped_ipv6_loopback() {
        // ::ffff:127.0.0.1
        let addr = IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped());
        assert!(is_private_ip(addr));
    }

    #[test]
    fn ipv4_mapped_ipv6_private() {
        // ::ffff:10.0.0.1
        let addr = IpAddr::V6(Ipv4Addr::new(10, 0, 0, 1).to_ipv6_mapped());
        assert!(is_private_ip(addr));
    }

    #[test]
    fn ipv4_mapped_ipv6_public() {
        // ::ffff:93.184.216.34
        let addr = IpAddr::V6(Ipv4Addr::new(93, 184, 216, 34).to_ipv6_mapped());
        assert!(!is_private_ip(addr));
    }

    #[test]
    fn ipv6_loopback() {
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn ipv6_public() {
        // 2606:4700::1 (Cloudflare)
        let addr: IpAddr = "2606:4700::1".parse().unwrap();
        assert!(!is_private_ip(addr));
    }
}
