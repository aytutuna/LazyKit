using System.Security.Cryptography;
using uniffi.lazy_link_ffi;

namespace LazyLink.RelayTests;

public class RelayTests
{
    // The development relay under `wrangler dev --local-protocol https`, whose certificate authority is in the Windows
    // root store, and its mail sink; the script that runs the tests expects both.
    private static readonly string Base = (Environment.GetEnvironmentVariable("RELAY_URL") ?? throw new InvalidOperationException("RELAY_URL is not set")).TrimEnd('/');
    private static readonly string AccountBase = Base.Replace("/relay/", "/account/");
    private static readonly string Sink = Environment.GetEnvironmentVariable("MAIL_SINK") ?? "http://127.0.0.1:8790";
    private const string Password = "correct horse battery";

    private static string NewId() => Convert.ToHexString(RandomNumberGenerator.GetBytes(16)).ToLowerInvariant();
    private static string NewRoom() => Convert.ToHexString(RandomNumberGenerator.GetBytes(32)).ToLowerInvariant();
    private static Login Who(string device) => new("lazysend", device, null);

    // The newest code the service mailed to an address, read from the dev sink.
    private static async Task<string> CodeFor(string email)
    {
        using var http = new HttpClient();
        for (int i = 0; i < 50; i++)
        {
            var res = await http.GetAsync($"{Sink}/code?to={Uri.EscapeDataString(email)}");
            if (res.IsSuccessStatusCode) return (await res.Content.ReadAsStringAsync()).Trim();
            await Task.Delay(100);
        }
        throw new TimeoutException($"no mail came for {email}");
    }

    private sealed record Person(string Email, string Device, Session Session)
    {
        public Relay RelayFor(string group, string? device = null, Session? session = null, AppAttester? attester = null) =>
            new(Base, group, device ?? Device, (session ?? Session).Token, attester);
    }

    // A new account, signed up through the service with the code from the dev sink.
    private static async Task<Person> SignUp()
    {
        string email = $"{NewId()[..12]}@example.com";
        string device = NewId();
        var account = new Account(AccountBase, null);
        await account.Signup(email, new Bot.Turnstile("dev"));
        var session = await account.ConfirmSignup(email, await CodeFor(email), Password, Who(device));
        return new Person(email, device, session);
    }

    private static async Task<Session> SignIn(string email, string device) =>
        await new Account(AccountBase, null).SigninPassword(email, Password, Who(device));

    [Fact]
    public async Task ASignedInAccountClaimsAGroupAndSeesItsAllowance()
    {
        var person = await SignUp();
        var relay = person.RelayFor(NewId());
        var claimed = await relay.Claim(GroupPlatform.Desktop);
        Assert.Equal(36, claimed.AccountToken.Length);
        var status = await relay.Status();
        Assert.Equal(0UL, status.used);
        Assert.Equal(5_000_000_000UL, status.cap);
        Assert.False(status.capped);
        Assert.Contains("\"email\":\"" + person.Email + "\"", person.Session.AccountJson);
    }

    [Fact]
    public async Task WrongPasswordsAndMissingSessionsAreRefusedByName()
    {
        var person = await SignUp();
        await Assert.ThrowsAsync<AccountFailure.WrongCredentials>(() =>
            new Account(AccountBase, null).SigninPassword(person.Email, "not the password", Who(NewId())));
        string group = NewId();
        await Assert.ThrowsAsync<RelayFailure.SignIn>(() => new Relay(Base, group, person.Device, "", null).Status());
    }

    [Fact]
    public async Task APhoneGroupNeedsItsProofAndNothingFlowsWithout()
    {
        var relay = (await SignUp()).RelayFor(NewId());
        await relay.Claim(GroupPlatform.Apple);
        await Assert.ThrowsAsync<RelayFailure.AttestationRequired>(() => relay.Connect(NewRoom()));
        await Assert.ThrowsAsync<RelayFailure.AttestationRequired>(() => relay.RegisterPush(PushPlatform.Fcm, "token"));
    }

    [Fact]
    public async Task TwoDevicesTalkThroughTheRelay()
    {
        var person = await SignUp();
        string group = NewId();
        string room = NewRoom();
        await person.RelayFor(group).Claim(GroupPlatform.Desktop);
        var host = await person.RelayFor(group).Host(room);

        string laptop = NewId();
        var guestSide = await person.RelayFor(group, laptop, await SignIn(person.Email, laptop)).Connect(room);
        var hostSide = await host.Accept();

        byte[] up = RandomNumberGenerator.GetBytes(300_000);
        byte[] down = RandomNumberGenerator.GetBytes(200_000);
        var send = Task.Run(async () =>
        {
            for (int at = 0; at < up.Length; at += 50_000) await guestSide.Write(up[at..Math.Min(at + 50_000, up.Length)]);
            await guestSide.Shutdown();
        });
        var got = new List<byte>();
        while (got.Count < up.Length) got.AddRange(await hostSide.Read(65_536));
        await send;
        Assert.Equal(up, got.ToArray());
        Assert.Empty(await hostSide.Read(1024));

        await hostSide.Write(down);
        await hostSide.Shutdown();
        var back = new List<byte>();
        while (true)
        {
            var part = await guestSide.Read(65_536);
            if (part.Length == 0) break;
            back.AddRange(part);
        }
        Assert.Equal(down, back.ToArray());

        await guestSide.Finish();
        await hostSide.Finish();
    }

    [Fact]
    public async Task AnotherAccountCannotUseTheGroup()
    {
        var mine = await SignUp();
        string group = NewId();
        await mine.RelayFor(group).Claim(GroupPlatform.Desktop);
        var other = await SignUp();
        await Assert.ThrowsAsync<RelayFailure.WrongAccount>(() => other.RelayFor(group).Status());
        // A session made for another device does not speak for this one.
        await Assert.ThrowsAsync<RelayFailure.WrongDevice>(() => mine.RelayFor(group, NewId()).Host(NewRoom()));
    }
}
