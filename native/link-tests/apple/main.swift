import Foundation

// Runs the account and relay clients, TLS and all, from Swift against a development relay whose certificate authority is
// in the system keychain, with its mail sink for the codes; scripts/link-swift.sh builds the bindings and passes
// RELAY_URL and MAIL_SINK.

var failures: [String] = []

func check(_ condition: Bool, _ message: @autoclosure () -> String) {
    if !condition { failures.append(message()) }
}

let env = ProcessInfo.processInfo.environment
let base = env["RELAY_URL"]!
let accountBase = base.replacingOccurrences(of: "/relay/", with: "/account/")
let sink = env["MAIL_SINK"] ?? "http://127.0.0.1:8790"
let password = "correct horse battery"

func newId() -> String { (0..<16).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined() }
func newRoom() -> String { newId() + newId() }
func who(_ device: String) -> Login { Login(app: "lazysend", device: device, label: nil) }

// The newest code the service mailed to an address, read from the dev sink.
func codeFor(_ email: String) async throws -> String {
    var allowed = CharacterSet.urlQueryAllowed
    allowed.remove(charactersIn: "+&=")
    let url = URL(string: "\(sink)/code?to=\(email.addingPercentEncoding(withAllowedCharacters: allowed)!)")!
    for _ in 0..<50 {
        let (data, response) = try await URLSession.shared.data(from: url)
        if (response as? HTTPURLResponse)?.statusCode == 200 {
            return String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        try await Task.sleep(nanoseconds: 100_000_000)
    }
    throw LinkError.Unexpected(reason: "no mail came for \(email)")
}

struct Person {
    let email: String
    let device: String
    let session: Session

    func relay(_ group: String, device: String? = nil, session: Session? = nil, attester: AppAttester? = nil) throws -> Relay {
        try Relay.newAt(base: base, group: group, device: device ?? self.device, session: (session ?? self.session).token, attester: attester)
    }
}

// A new account, signed up through the service with the code from the dev sink.
func signUp() async throws -> Person {
    let email = "\(newId().prefix(12))@example.com"
    let device = newId()
    let account = try Account.newAt(base: accountBase, session: nil)
    try await account.signup(email: email, botCheck: .turnstile(token: "dev"))
    let session = try await account.confirmSignup(email: email, code: try await codeFor(email), password: password, who: who(device))
    return Person(email: email, device: device, session: session)
}

func signIn(_ email: String, _ device: String) async throws -> Session {
    try await Account.newAt(base: accountBase, session: nil).signinPassword(email: email, password: password, who: who(device))
}

func claimAndStatus() async {
    do {
        let person = try await signUp()
        let relay = try person.relay(newId())
        let claimed = try await relay.claim(platform: .desktop)
        check(claimed.accountToken.count == 36, "the account token comes with the claim")
        let s = try await relay.status()
        check(s.used == 0 && s.cap == 5_000_000_000 && !s.capped, "a fresh group is empty and under the cap")
        check(person.session.accountJson.contains("\"email\":\"\(person.email)\""), "the session carries the account")
    } catch {
        check(false, "claim and status: \(error)")
    }
}

func refusedByName() async {
    do {
        let person = try await signUp()
        do {
            _ = try await Account.newAt(base: accountBase, session: nil).signinPassword(email: person.email, password: "not the password", who: who(newId()))
            check(false, "a wrong password was accepted")
        } catch AccountFailure.WrongCredentials {
        }
        do {
            _ = try await Relay.newAt(base: base, group: newId(), device: person.device, session: "", attester: nil).status()
            check(false, "a request without a session was answered")
        } catch RelayFailure.SignIn {
        }
    } catch {
        check(false, "refused by name: \(error)")
    }
}

func phoneGroupNeedsItsProof() async {
    do {
        let relay = try await signUp().relay(newId())
        _ = try await relay.claim(platform: .apple)
        do {
            _ = try await relay.connect(room: newRoom())
            check(false, "a room was opened without a proof")
        } catch RelayFailure.AttestationRequired {
        }
        do {
            try await relay.registerPush(platform: .fcm, token: "token")
            check(false, "a push token was kept without a proof")
        } catch RelayFailure.AttestationRequired {
        }
    } catch {
        check(false, "phone group: \(error)")
    }
}

func twoDevices() async {
    do {
        let person = try await signUp()
        let group = newId()
        let room = newRoom()
        _ = try await person.relay(group).claim(platform: .desktop)
        let hosting = try await person.relay(group).host(room: room)
        let laptop = newId()
        let guestSide = try await person.relay(group, device: laptop, session: try await signIn(person.email, laptop)).connect(room: room)
        let hostSide = try await hosting.accept()

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

func otherAccount() async {
    do {
        let mine = try await signUp()
        let group = newId()
        _ = try await mine.relay(group).claim(platform: .desktop)
        let other = try await signUp()
        do {
            _ = try await other.relay(group).status()
            check(false, "another account used the group")
        } catch RelayFailure.WrongAccount {
        }
        // A session made for another device does not speak for this one.
        do {
            _ = try await mine.relay(group, device: newId()).host(room: newRoom())
            check(false, "a session was used on another device")
        } catch RelayFailure.WrongDevice {
        }
    } catch {
        check(false, "other account: \(error)")
    }
}

await claimAndStatus()
await refusedByName()
await phoneGroupNeedsItsProof()
await twoDevices()
await otherAccount()

if failures.isEmpty {
    print("ok")
} else {
    for f in failures { print("FAIL: \(f)") }
    exit(1)
}
