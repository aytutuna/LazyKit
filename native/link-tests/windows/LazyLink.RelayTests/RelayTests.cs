using System.Security.Cryptography;
using System.Text;
using uniffi.lazy_link_ffi;

namespace LazyLink.RelayTests;

// The desktop proof the relay asks of a computer: a key this install made, signing the group and the challenge.
internal sealed class InstallKey : AppAttester
{
    private readonly ECDsa key = ECDsa.Create(ECCurve.NamedCurves.nistP256);
    public int Calls;

    public Task<AppProof> Attest(string group, string challenge)
    {
        Interlocked.Increment(ref Calls);
        var message = Encoding.UTF8.GetBytes($"lazy relay install v1\0{group}\0{challenge}");
        var point = key.ExportParameters(false).Q;
        byte[] publicKey = [4, .. point.X!, .. point.Y!];
        return Task.FromResult<AppProof>(new AppProof.Desktop(publicKey, key.SignData(message, HashAlgorithmName.SHA256)));
    }
}

public class RelayTests
{
    // The development relay under `wrangler dev --local-protocol https`, whose certificate authority is in the Windows
    // root store; the script that runs the tests expects both.
    private static readonly Uri Url = new(Environment.GetEnvironmentVariable("RELAY_URL") ?? throw new InvalidOperationException("RELAY_URL is not set"));
    private static string Base => Url.ToString().TrimEnd('/');
    private static string NewId() => Convert.ToHexString(RandomNumberGenerator.GetBytes(16)).ToLowerInvariant();
    private static string NewRoom() => Convert.ToHexString(RandomNumberGenerator.GetBytes(32)).ToLowerInvariant();
    private static Relay NewRelay(string? group = null, AppAttester? attester = null) =>
        new(Base, group ?? NewId(), NewId(), attester);

    [Fact]
    public async Task GroupCallsRunOverRustTls()
    {
        var status = await NewRelay().Status();
        Assert.Equal(0UL, status.Used);
        Assert.Equal(5_000_000_000UL, status.Cap);
        Assert.False(status.Capped);
    }

    [Fact]
    public async Task ANewGroupNeedsItsProofAndNothingFlowsWithout()
    {
        await Assert.ThrowsAsync<RelayFailure.AttestationRequired>(() => NewRelay().Connect(NewRoom()));
        await Assert.ThrowsAsync<RelayFailure.AttestationRequired>(() => NewRelay().RegisterPush(PushPlatform.Fcm, "token"));
    }

    [Fact]
    public async Task TwoDevicesTalkThroughTheRelay()
    {
        string group = NewId();
        string room = NewRoom();
        var hostKey = new InstallKey();
        var host = await NewRelay(group, hostKey).Host(room);
        Assert.Equal(1, hostKey.Calls);

        var guestKey = new InstallKey();
        var guestSide = await NewRelay(group, guestKey).Connect(room);
        var hostSide = await host.Accept();
        // The group is verified already, so the second device is not asked again.
        Assert.Equal(0, guestKey.Calls);

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
    public async Task AnotherGroupCannotUseTheRoom()
    {
        string room = NewRoom();
        var host = await NewRelay(NewId(), new InstallKey()).Host(room);
        await Assert.ThrowsAsync<RelayFailure.WrongGroup>(() => NewRelay(NewId(), new InstallKey()).Connect(room));
        GC.KeepAlive(host);
    }
}
