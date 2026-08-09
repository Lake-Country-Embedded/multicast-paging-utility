//! Address range parsing for multicast endpoint specification.

#![allow(dead_code)]

use std::net::Ipv4Addr;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum RangeParseError {
    #[error("Invalid IP address format")]
    InvalidIpFormat,

    #[error("Missing port in address specification")]
    MissingPort,

    #[error("Invalid port: {0}")]
    InvalidPort(String),

    #[error("Range {0}-{1} is invalid (start > end)")]
    InvalidRange(u32, u32),

    #[error("Port must be between 1024 and 65535, got {0}")]
    PortOutOfRange(u32),

    #[error("IP octet must be between 0 and 255, got {0}")]
    OctetOutOfRange(u32),

    #[error("Address {0} is not a valid multicast address (must be 224.0.0.0 - 239.255.255.255)")]
    NotMulticast(Ipv4Addr),

    #[error("Syntax error: {0}")]
    SyntaxError(String),
}

/// A parsed multicast endpoint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MulticastEndpoint {
    pub address: Ipv4Addr,
    pub port: u16,
}

impl std::fmt::Display for MulticastEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.address, self.port)
    }
}

/// Parses address range patterns and expands them into a list of endpoints
///
/// Supported patterns:
/// - `224.0.1.1:5004` - Single address
/// - `224.0.{0-10}.{0-10}:5004` - Range in octets
/// - `224.0.1.1:{5004-5014}` - Range in port
/// - `224.0.{1-5}.1:{5004-5008}` - Combined ranges
/// - `224.0.1.1:5004,224.0.1.5:5004` - Comma-separated list of any of the above
///
/// The comma form matters for callers with an arbitrary, non-contiguous set of
/// endpoints: a range can only express a run, so such callers previously had to
/// either widen the range (joining groups nobody asked for, and counting pages
/// on them) or emit a comma list that this parser rejected outright.
pub fn parse_range(pattern: &str) -> Result<Vec<MulticastEndpoint>, RangeParseError> {
    let pattern = pattern.trim();

    // A comma-separated list is the union of its parts, de-duplicated so an
    // overlapping range and literal do not double-count an endpoint.
    if pattern.contains(',') {
        let mut endpoints: Vec<MulticastEndpoint> = Vec::new();
        for part in pattern.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            for endpoint in parse_range(part)? {
                if !endpoints.contains(&endpoint) {
                    endpoints.push(endpoint);
                }
            }
        }
        if endpoints.is_empty() {
            return Err(RangeParseError::InvalidIpFormat);
        }
        return Ok(endpoints);
    }

    // Split into address and port parts
    let (addr_part, port_part) = pattern
        .rsplit_once(':')
        .ok_or(RangeParseError::MissingPort)?;

    // Parse port(s)
    let ports = parse_range_component(port_part)?;
    for &port in &ports {
        if !(1024..=65535).contains(&port) {
            return Err(RangeParseError::PortOutOfRange(port));
        }
    }

    // Parse octets
    let octets: Vec<&str> = addr_part.split('.').collect();
    if octets.len() != 4 {
        return Err(RangeParseError::InvalidIpFormat);
    }

    let mut octet_ranges: Vec<Vec<u32>> = Vec::with_capacity(4);
    for octet in octets {
        let range = parse_range_component(octet)?;
        for &val in &range {
            if val > 255 {
                return Err(RangeParseError::OctetOutOfRange(val));
            }
        }
        octet_ranges.push(range);
    }

    // Generate all combinations (Cartesian product)
    let mut endpoints = Vec::new();
    for &o1 in &octet_ranges[0] {
        for &o2 in &octet_ranges[1] {
            for &o3 in &octet_ranges[2] {
                for &o4 in &octet_ranges[3] {
                    let addr = Ipv4Addr::new(o1 as u8, o2 as u8, o3 as u8, o4 as u8);

                    // Validate multicast range (224.0.0.0 - 239.255.255.255)
                    if !addr.is_multicast() {
                        return Err(RangeParseError::NotMulticast(addr));
                    }

                    for &port in &ports {
                        endpoints.push(MulticastEndpoint {
                            address: addr,
                            port: port as u16,
                        });
                    }
                }
            }
        }
    }

    Ok(endpoints)
}

/// Parses a range component like "{0-10}" or "5004" into a Vec of values
fn parse_range_component(s: &str) -> Result<Vec<u32>, RangeParseError> {
    let s = s.trim();

    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len() - 1];
        let (start_str, end_str) = inner
            .split_once('-')
            .ok_or_else(|| RangeParseError::SyntaxError("Invalid range syntax, expected {start-end}".into()))?;

        let start: u32 = start_str
            .trim()
            .parse()
            .map_err(|_| RangeParseError::SyntaxError(format!("Invalid number: {}", start_str)))?;

        let end: u32 = end_str
            .trim()
            .parse()
            .map_err(|_| RangeParseError::SyntaxError(format!("Invalid number: {}", end_str)))?;

        if start > end {
            return Err(RangeParseError::InvalidRange(start, end));
        }

        Ok((start..=end).collect())
    } else {
        let val: u32 = s
            .parse()
            .map_err(|_| RangeParseError::SyntaxError(format!("Invalid number: {}", s)))?;
        Ok(vec![val])
    }
}

/// Count the number of endpoints that would be generated by a pattern
///
/// For comma-separated patterns this defers to [`parse_range`] so the count
/// matches what would actually be monitored, de-duplication included. The
/// arithmetic fast path below only holds for a single range.
pub fn count_endpoints(pattern: &str) -> Result<usize, RangeParseError> {
    let pattern = pattern.trim();

    if pattern.contains(',') {
        return Ok(parse_range(pattern)?.len());
    }

    let (addr_part, port_part) = pattern
        .rsplit_once(':')
        .ok_or(RangeParseError::MissingPort)?;

    let port_count = parse_range_component(port_part)?.len();

    let octets: Vec<&str> = addr_part.split('.').collect();
    if octets.len() != 4 {
        return Err(RangeParseError::InvalidIpFormat);
    }

    let mut total = port_count;
    for octet in octets {
        total *= parse_range_component(octet)?.len();
    }

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_address() {
        let endpoints = parse_range("224.0.1.1:5004").unwrap();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].address, Ipv4Addr::new(224, 0, 1, 1));
        assert_eq!(endpoints[0].port, 5004);
    }

    #[test]
    fn test_parse_octet_range() {
        let endpoints = parse_range("224.0.{1-3}.1:5004").unwrap();
        assert_eq!(endpoints.len(), 3);
        assert_eq!(endpoints[0].address, Ipv4Addr::new(224, 0, 1, 1));
        assert_eq!(endpoints[1].address, Ipv4Addr::new(224, 0, 2, 1));
        assert_eq!(endpoints[2].address, Ipv4Addr::new(224, 0, 3, 1));
    }

    #[test]
    fn test_parse_port_range() {
        let endpoints = parse_range("224.0.1.1:{5004-5006}").unwrap();
        assert_eq!(endpoints.len(), 3);
        assert_eq!(endpoints[0].port, 5004);
        assert_eq!(endpoints[1].port, 5005);
        assert_eq!(endpoints[2].port, 5006);
    }

    #[test]
    fn test_parse_combined_ranges() {
        let endpoints = parse_range("224.0.{0-1}.{0-1}:5004").unwrap();
        assert_eq!(endpoints.len(), 4); // 2 * 2 = 4
    }

    #[test]
    fn test_parse_large_range() {
        let endpoints = parse_range("224.0.{0-10}.{0-10}:5004").unwrap();
        assert_eq!(endpoints.len(), 121); // 11 * 11 = 121
    }

    #[test]
    fn test_invalid_multicast_address() {
        let result = parse_range("192.168.1.1:5004");
        assert!(matches!(result, Err(RangeParseError::NotMulticast(_))));
    }

    #[test]
    fn test_invalid_range_syntax() {
        let result = parse_range("224.0.1.1");
        assert!(matches!(result, Err(RangeParseError::MissingPort)));
    }

    #[test]
    fn test_invalid_range_order() {
        let result = parse_range("224.0.{10-5}.1:5004");
        assert!(matches!(result, Err(RangeParseError::InvalidRange(10, 5))));
    }

    #[test]
    fn test_port_out_of_range() {
        let result = parse_range("224.0.1.1:80");
        assert!(matches!(result, Err(RangeParseError::PortOutOfRange(80))));
    }

    #[test]
    fn test_octet_out_of_range() {
        let result = parse_range("224.0.{250-260}.1:5004");
        assert!(matches!(result, Err(RangeParseError::OctetOutOfRange(256))));
    }

    #[test]
    fn test_count_endpoints() {
        assert_eq!(count_endpoints("224.0.1.1:5004").unwrap(), 1);
        assert_eq!(count_endpoints("224.0.{0-10}.{0-10}:5004").unwrap(), 121);
        assert_eq!(count_endpoints("224.0.1.1:{5000-5009}").unwrap(), 10);
    }

    #[test]
    fn test_comma_separated_list() {
        // The case a range cannot express: two non-adjacent groups.
        let endpoints = parse_range("224.0.1.1:5004,224.0.1.5:5004").unwrap();
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].address, Ipv4Addr::new(224, 0, 1, 1));
        assert_eq!(endpoints[1].address, Ipv4Addr::new(224, 0, 1, 5));
    }

    #[test]
    fn test_comma_separated_mixes_ranges_and_literals() {
        let endpoints = parse_range("224.0.1.{1-3}:5004,224.0.2.9:5005").unwrap();
        assert_eq!(endpoints.len(), 4);
        assert_eq!(endpoints[3].address, Ipv4Addr::new(224, 0, 2, 9));
        assert_eq!(endpoints[3].port, 5005);
    }

    #[test]
    fn test_comma_separated_deduplicates() {
        // An overlapping range and literal must not count an endpoint twice --
        // a duplicate would mean two sockets joining the same group.
        let endpoints = parse_range("224.0.1.{1-3}:5004,224.0.1.2:5004").unwrap();
        assert_eq!(endpoints.len(), 3);
    }

    #[test]
    fn test_comma_separated_tolerates_spacing_and_trailing_commas() {
        let endpoints = parse_range("224.0.1.1:5004, 224.0.1.2:5004,").unwrap();
        assert_eq!(endpoints.len(), 2);
    }

    #[test]
    fn test_comma_separated_propagates_a_bad_part() {
        // One invalid member fails the whole pattern rather than being skipped:
        // silently monitoring fewer groups than asked for is how a paging test
        // passes while watching nothing.
        assert!(parse_range("224.0.1.1:5004,10.0.0.1:5004").is_err());
        assert!(parse_range("224.0.1.1:5004,garbage").is_err());
    }

    #[test]
    fn test_display() {
        let endpoint = MulticastEndpoint {
            address: Ipv4Addr::new(224, 0, 1, 1),
            port: 5004,
        };
        assert_eq!(format!("{}", endpoint), "224.0.1.1:5004");
    }
}
