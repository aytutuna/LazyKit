import CryptoKit
import Foundation

// Runs the relay client, TLS and all, from Swift against a development relay whose certificate authority is in the system
// keychain; scripts/link-swift.sh builds the bindings and passes RELAY_URL.

var failures: [String] = []

func check(_ condition: Bool, _ message: @autoclosure () -> String) {
    if !condition { failures.append(message()) }
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

func newId() -> String { (0..<16).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined() }
func newRoom() -> String { newId() + newId() }
func relay(group: String? = nil, attester: AppAttester? = nil) throws -> Relay {
    try Relay(base: base, group: group ?? newId(), device: newId(), attester: attester)
}

func groupCalls() async {
    do {
        let s = try await relay().status()
        check(s.used == 0 && s.cap == 5_000_000_000 && !s.capped, "a fresh group is empty and under the cap")
    } catch {
        check(false, "status: \(error)")
    }
}

func proofIsRequired() async {
    do {
        _ = try await relay().connect(room: newRoom())
        check(false, "a room was opened without a proof")
    } catch RelayFailure.AttestationRequired {
    } catch {
        check(false, "connect without proof: \(error)")
    }
    do {
        try await relay().registerPush(platform: .fcm, token: "token")
        check(false, "a push token was kept without a proof")
    } catch RelayFailure.AttestationRequired {
    } catch {
        check(false, "push without proof: \(error)")
    }
}

func twoDevices() async {
    do {
        let group = newId()
        let room = newRoom()
        let hostKey = InstallKey()
        let hosting = try await relay(group: group, attester: hostKey).host(room: room)
        check(hostKey.calls == 1, "the first device proves itself once")
        let guestKey = InstallKey()
        let guestSide = try await relay(group: group, attester: guestKey).connect(room: room)
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
    do {
        let room = newRoom()
        let hosting = try await relay(attester: InstallKey()).host(room: room)
        do {
            _ = try await relay(attester: InstallKey()).connect(room: room)
            check(false, "another group joined the room")
        } catch RelayFailure.WrongGroup {
        }
        _ = hosting
    } catch {
        check(false, "other group: \(error)")
    }
}

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
