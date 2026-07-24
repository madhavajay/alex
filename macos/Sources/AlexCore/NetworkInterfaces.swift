import Foundation

#if os(Linux)
import Glibc
#else
import Darwin
#endif

public struct NetworkInterfaceAddress: Identifiable, Sendable, Hashable {
    public let name: String
    public let address: String

    public var id: String { "\(name):\(address)" }
    public var displayName: String { "\(NetworkInterfaces.friendlyName(name, address: address)) (\(address))" }
}

/// Local addresses suitable for choosing a daemon listener. This intentionally
/// omits loopback and IPv6 link-local addresses: the latter need a scope ID and
/// are not a durable address to save in config.toml.
public enum NetworkInterfaces {
    public static func addresses() -> [NetworkInterfaceAddress] {
        var first: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&first) == 0, let first else { return [] }
        defer { freeifaddrs(first) }

        var result: [NetworkInterfaceAddress] = []
        var cursor: UnsafeMutablePointer<ifaddrs>? = first
        while let entry = cursor {
            defer { cursor = entry.pointee.ifa_next }
            guard let sockaddr = entry.pointee.ifa_addr,
                  let address = numericAddress(sockaddr),
                  !isLoopback(address),
                  !address.lowercased().hasPrefix("fe80:") else { continue }
            let name = String(cString: entry.pointee.ifa_name)
            result.append(NetworkInterfaceAddress(name: name, address: address))
        }
        return Array(Set(result)).sorted { lhs, rhs in
            lhs.displayName.localizedStandardCompare(rhs.displayName) == .orderedAscending
        }
    }

    public static func friendlyName(_ name: String, address: String) -> String {
        if name.lowercased().contains("tailscale") || isTailscale(address) { return "Tailscale" }
        if name == "en0" { return "Wi-Fi" }
        if name.hasPrefix("en") { return "Ethernet (\(name))" }
        if name.hasPrefix("bridge") { return "Bridge (\(name))" }
        if name.hasPrefix("utun") { return "VPN (\(name))" }
        return "Interface \(name)"
    }

    /// Orders addresses by how likely a remote machine is to reach them:
    /// primary LAN interfaces first, then Tailscale, then virtual interfaces.
    /// Alphabetical `addresses()` order puts bridges and VPN tunnels ahead of
    /// Wi-Fi, which is how remote 1-liners ended up embedding unreachable IPs.
    public static func rankedForRemoteAccess(
        _ addresses: [NetworkInterfaceAddress]
    ) -> [NetworkInterfaceAddress] {
        addresses.sorted { lhs, rhs in
            let lhsRank = remoteAccessRank(lhs)
            let rhsRank = remoteAccessRank(rhs)
            if lhsRank != rhsRank { return lhsRank < rhsRank }
            return lhs.displayName.localizedStandardCompare(rhs.displayName)
                == .orderedAscending
        }
    }

    private static func remoteAccessRank(_ interface: NetworkInterfaceAddress) -> Int {
        let ipv4Penalty = interface.address.contains(":") ? 1 : 0
        if interface.name == "en0" { return 0 + ipv4Penalty }
        if isTailscale(interface.address)
            || interface.name.lowercased().contains("tailscale")
        {
            return 4 + ipv4Penalty
        }
        if interface.name.hasPrefix("en") { return 2 + ipv4Penalty }
        if interface.name.hasPrefix("bridge") || interface.name.hasPrefix("utun") {
            return 6 + ipv4Penalty
        }
        return 8 + ipv4Penalty
    }

    private static func numericAddress(_ address: UnsafeMutablePointer<sockaddr>) -> String? {
        switch Int32(address.pointee.sa_family) {
        case AF_INET:
            var value = address.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { $0.pointee.sin_addr }
            var buffer = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
            guard inet_ntop(AF_INET, &value, &buffer, socklen_t(buffer.count)) != nil else { return nil }
            return string(from: buffer)
        case AF_INET6:
            var value = address.withMemoryRebound(to: sockaddr_in6.self, capacity: 1) { $0.pointee.sin6_addr }
            var buffer = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))
            guard inet_ntop(AF_INET6, &value, &buffer, socklen_t(buffer.count)) != nil else { return nil }
            return string(from: buffer)
        default:
            return nil
        }
    }

    private static func string(from buffer: [CChar]) -> String {
        String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }

    private static func isLoopback(_ address: String) -> Bool {
        address == "::1" || address.hasPrefix("127.")
    }

    private static func isTailscale(_ address: String) -> Bool {
        let octets = address.split(separator: ".").compactMap { Int($0) }
        return octets.count == 4 && octets[0] == 100 && (64...127).contains(octets[1])
    }
}
