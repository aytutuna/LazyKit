using System.Collections.Concurrent;
using System.Net.Security;
using System.Net.Sockets;
using System.Security.Cryptography.X509Certificates;
using uniffi.lazy_link_ffi;

namespace LazyLink.TransportTests;

// What a Windows app gives the relay client: SslStream does the TLS with the system trust store. The tests talk to a
// development relay with a self-signed certificate, so they can ask it to accept any certificate.
internal sealed class SslTransport(bool acceptAnyCertificate) : SecureTransport
{
    private sealed record Conn(TcpClient Tcp, SslStream Ssl);

    private readonly ConcurrentDictionary<ulong, Conn> conns = new();
    private long last;

    public int Open => conns.Count;

    public async Task<ulong> Connect(string host, ushort port, string serverName, bool pinned)
    {
        var tcp = new TcpClient();
        try
        {
            await tcp.ConnectAsync(host, port);
            var ssl = new SslStream(tcp.GetStream(), false, (_, _, _, errors) => acceptAnyCertificate || pinned || errors == SslPolicyErrors.None);
            await ssl.AuthenticateAsClientAsync(serverName);
            ulong id = (ulong)Interlocked.Increment(ref last);
            conns[id] = new Conn(tcp, ssl);
            return id;
        }
        catch (Exception e)
        {
            tcp.Dispose();
            throw new LinkException.Unexpected(e.Message);
        }
    }

    public async Task<byte[]> Read(ulong connection, uint max)
    {
        if (!conns.TryGetValue(connection, out var c)) return [];
        try
        {
            var buf = new byte[max];
            int n = await c.Ssl.ReadAsync(buf);
            return buf[..n];
        }
        catch (Exception e) when (e is IOException or ObjectDisposedException)
        {
            return [];
        }
    }

    public async Task Write(ulong connection, byte[] data)
    {
        if (!conns.TryGetValue(connection, out var c)) throw new LinkException.Unexpected("closed");
        try
        {
            await c.Ssl.WriteAsync(data);
            await c.Ssl.FlushAsync();
        }
        catch (Exception e) when (e is IOException or ObjectDisposedException)
        {
            throw new LinkException.Unexpected(e.Message);
        }
    }

    public Task Close(ulong connection)
    {
        if (conns.TryRemove(connection, out var c))
        {
            c.Ssl.Dispose();
            c.Tcp.Dispose();
        }
        return Task.CompletedTask;
    }

    public byte[]? PeerSpki(ulong connection) =>
        conns.TryGetValue(connection, out var c) && c.Ssl.RemoteCertificate is { } cert
            ? new X509Certificate2(cert).PublicKey.ExportSubjectPublicKeyInfo()
            : null;
}
