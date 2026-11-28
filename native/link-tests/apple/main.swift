import CryptoKit
import Foundation
import Network
import Security

// Runs the relay client through Network.framework, the transport the Apple apps use, against a development relay with a
// self-signed certificate; scripts/link-swift.sh builds the bindings and passes RELAY_URL.

var failures: [String] = []

func check(_ condition: Bool, _ message: @autoclosure () -> String) {
    if !condition { failures.append(message()) }
}

// The public key of the first certificate in DER (a SubjectPublicKeyInfo), read out of the certificate itself since
// SecKey only gives the bare key.
func spki(of data: Data) -> Data? {
    let der = Data(data)
    func element(_ d: Data, _ at: Int) -> (tag: UInt8, body: Range<Int>)? {
        guard at + 2 <= d.count else { return nil }
        var length = Int(d[at + 1])
        var start = at + 2
        if length & 0x80 != 0 {
            let n = length & 0x7f
            guard n > 0, n <= 4, start + n <= d.count else { return nil }
            length = 0
            for i in 0..<n { length = length * 256 + Int(d[start + i]) }
            start += n
        }
        guard start + length <= d.count else { return nil }
        return (d[at], start..<(start + length))
    }
    guard let cert = element(der, 0), let tbs = element(der, cert.body.lowerBound) else { return nil }
    var items: [(start: Int, end: Int, tag: UInt8)] = []
    var at = tbs.body.lowerBound
    while at < tbs.body.upperBound, let e = element(der, at) {
        items.append((at, e.body.upperBound, e.tag))
        at = e.body.upperBound
    }
    // an optional version, then serial, algorithm, issuer, validity, subject and the key
    let first = items.first?.tag == 0xa0 ? 1 : 0
    guard items.count > first + 5 else { return nil }
    let key = items[first + 5]
    return Data(der[key.start..<key.end])
}

final class Connection: @unchecked Sendable {
    let nw: NWConnection
    var certificate: Data?
    init(_ nw: NWConnection) { self.nw = nw }
}

final class NetworkTransport: SecureTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var connections: [UInt64: Connection] = [:]
    private var last: UInt64 = 0
    private let acceptAny: Bool

    init(acceptAnyCertificate: Bool) { acceptAny = acceptAnyCertificate }

    var open: Int { lock.withLock { connections.count } }

    private func find(_ id: UInt64) -> Connection? { lock.withLock { connections[id] } }

    func connect(host: String, port: UInt16, serverName: String, pinned: Bool) async throws -> UInt64 {
        let tls = NWProtocolTLS.Options()
        let box = Box()
        sec_protocol_options_set_tls_server_name(tls.securityProtocolOptions, serverName)
        sec_protocol_options_set_verify_block(tls.securityProtocolOptions, { metadata, trust, complete in
            sec_protocol_metadata_access_peer_certificate_chain(metadata) { secCert in
                if box.certificate == nil {
                    box.certificate = SecCertificateCopyData(sec_certificate_copy_ref(secCert).takeRetainedValue()) as Data
                }
            }
            if self.acceptAny || pinned {
                complete(true)
            } else {
                complete(SecTrustEvaluateWithError(sec_trust_copy_ref(trust).takeRetainedValue(), nil))
            }
        }, DispatchQueue(label: "verify"))
        let nw = NWConnection(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: port)!, using: NWParameters(tls: tls))
        let conn = Connection(nw)
        try await withCheckedThrowingContinuation { (k: CheckedContinuation<Void, Error>) in
            let once = Once()
            nw.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    if once.first() { k.resume() }
                case .failed(let e), .waiting(let e):
                    if once.first() {
                        nw.cancel()
                        k.resume(throwing: LinkError.Unexpected(reason: "\(e)"))
                    }
                case .cancelled:
                    if once.first() { k.resume(throwing: LinkError.Unexpected(reason: "cancelled")) }
                default:
                    break
                }
            }
            nw.start(queue: DispatchQueue(label: "link"))
        }
        conn.certificate = box.certificate
        return lock.withLock {
            last += 1
            connections[last] = conn
            return last
        }
    }

    func read(connection: UInt64, max: UInt32) async throws -> Data {
        guard let c = find(connection) else { return Data() }
        return await withCheckedContinuation { k in
            c.nw.receive(minimumIncompleteLength: 1, maximumLength: Int(max)) { data, _, _, error in
                k.resume(returning: error == nil ? (data ?? Data()) : Data())
            }
        }
    }

    func write(connection: UInt64, data: Data) async throws {
        guard let c = find(connection) else { throw LinkError.Unexpected(reason: "closed") }
        try await withCheckedThrowingContinuation { (k: CheckedContinuation<Void, Error>) in
            c.nw.send(content: data, completion: .contentProcessed { error in
                if let error { k.resume(throwing: LinkError.Unexpected(reason: "\(error)")) } else { k.resume() }
            })
        }
    }

    func close(connection: UInt64) async {
        let c = lock.withLock { connections.removeValue(forKey: connection) }
        c?.nw.cancel()
    }

    func peerSpki(connection: UInt64) -> Data? {
        find(connection)?.certificate.flatMap { spki(of: $0) }
    }
}

final class Box: @unchecked Sendable { var certificate: Data? }

final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    func first() -> Bool { lock.withLock { defer { done = true }; return !done } }
}

// The desktop proof the relay asks of a computer: a key this install made, signing the group and the challenge.
final class InstallKey: AppAttester, @unchecked Sendable {
    private let key = P256.Signing.PrivateKey()
    private let lock = NSLock()
    private var count = 0
    var calls: Int { lock.withLock { count } }

    func attest(group: String, challenge: String) async throws -> AppProof {
        lock.withLock { count += 1 }
        var message = Data("lazy relay install v1".utf8)
        message.append(0)
        message.append(Data(group.utf8))
        message.append(0)
        message.append(Data(challenge.utf8))
        let signature = try key.signature(for: message)
        return .desktop(publicKey: key.publicKey.x963Representation, signature: signature.rawRepresentation)
    }
}

let base = ProcessInfo.processInfo.environment["RELAY_URL"]!
let url = URL(string: base)!
let host = url.host!
let port = UInt16(url.port!)

func newId() -> String { (0..<16).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined() }
func newRoom() -> String { newId() + newId() }
func relay(_ t: SecureTransport, group: String? = nil, attester: AppAttester? = nil) -> Relay {
    Relay(base: base, group: group ?? newId(), device: newId(), transport: t, attester: attester)
}

func pins() async {
    let t = NetworkTransport(acceptAnyCertificate: true)
    do {
        let fp = try await probeConnection(transport: t, host: host, port: port, serverName: host, pin: nil)
        check(fp.count == 32, "fingerprint is 32 bytes")
        let again = try await probeConnection(transport: t, host: host, port: port, serverName: host, pin: fp)
        check(again == fp, "the pin that fits is accepted")
        var wrong = fp
        wrong[0] ^= 1
        do {
            _ = try await probeConnection(transport: t, host: host, port: port, serverName: host, pin: wrong)
            check(false, "a wrong pin was accepted")
        } catch {}
        check(t.open == 0, "probes close their connections")
    } catch {
        check(false, "probe: \(error)")
    }
}

func untrusted() async {
    let t = NetworkTransport(acceptAnyCertificate: false)
    do {
        _ = try await relay(t).status()
        check(false, "an untrusted certificate was accepted")
    } catch RelayFailure.Transport {
    } catch {
        check(false, "untrusted: \(error)")
    }
}

func groupCalls() async {
    do {
        let s = try await relay(NetworkTransport(acceptAnyCertificate: true)).status()
        check(s.used == 0 && s.cap == 5_000_000_000 && !s.capped, "a fresh group is empty and under the cap")
    } catch {
        check(false, "status: \(error)")
    }
}

func proofIsRequired() async {
    let t = NetworkTransport(acceptAnyCertificate: true)
    do {
        _ = try await relay(t).connect(room: newRoom())
        check(false, "a room was opened without a proof")
    } catch RelayFailure.AttestationRequired {
    } catch {
        check(false, "connect without proof: \(error)")
    }
    do {
        try await relay(t).registerPush(platform: .fcm, token: "token")
        check(false, "a push token was kept without a proof")
    } catch RelayFailure.AttestationRequired {
    } catch {
        check(false, "push without proof: \(error)")
    }
}

func twoDevices() async {
    let t = NetworkTransport(acceptAnyCertificate: true)
    do {
        let group = newId()
        let room = newRoom()
        let hostKey = InstallKey()
        let hosting = try await relay(t, group: group, attester: hostKey).host(room: room)
        check(hostKey.calls == 1, "the first device proves itself once")
        let guestKey = InstallKey()
        let guestSide = try await relay(t, group: group, attester: guestKey).connect(room: room)
        let hostSide = try await hosting.accept()
        check(guestKey.calls == 0, "a verified group is not asked again")

        let up = Data((0..<300_000).map { _ in UInt8.random(in: 0...255) })
        let down = Data((0..<200_000).map { _ in UInt8.random(in: 0...255) })
        let sending = Task {
            var at = 0
            while at < up.count {
                try await guestSide.write(data: up[at..<min(at + 50_000, up.count)])
                at += 50_000
            }
            try await guestSide.shutdown()
        }
        var got = Data()
        while got.count < up.count { got.append(try await hostSide.read(max: 65_536)) }
        try await sending.value
        check(got == up, "the host got what the guest sent")
        check(try await hostSide.read(max: 1024).isEmpty, "the guest's end of input reads as empty")

        try await hostSide.write(data: down)
        try await hostSide.shutdown()
        var back = Data()
        while true {
            let part = try await guestSide.read(max: 65_536)
            if part.isEmpty { break }
            back.append(part)
        }
        check(back == down, "the guest got the reply")
        try await guestSide.finish()
        try await hostSide.finish()
        _ = hosting
    } catch {
        check(false, "two devices: \(error)")
    }
}

func otherGroup() async {
    let t = NetworkTransport(acceptAnyCertificate: true)
    do {
        let room = newRoom()
        let hosting = try await relay(t, attester: InstallKey()).host(room: room)
        do {
            _ = try await relay(t, attester: InstallKey()).connect(room: room)
            check(false, "another group joined the room")
        } catch RelayFailure.WrongGroup {
        }
        _ = hosting
    } catch {
        check(false, "other group: \(error)")
    }
}

await pins()
await untrusted()
await groupCalls()
await proofIsRequired()
await twoDevices()
await otherGroup()

if failures.isEmpty {
    print("ok")
} else {
    for f in failures { print("FAIL: \(f)") }
    exit(1)
}
