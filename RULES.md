## Relay

Paired devices that cannot reach each other directly talk through the Lazy relay, a Cloudflare Worker at alazy.app/relay/. Devices on the same network connect directly and never touch it.

Rooms. A room is named by the lowercase hex SHA-256 of `lazy-relay-room-v1:` followed by a secret both devices hold: for paired devices the SHA-256 of the device secret (the host keeps only that hash, so both sides can work it out), for pairing by link or QR the 16-byte token, for pairing by short code the 8 canonical characters. The relay only ever sees the room name. Knowing a room name does not let anyone in on its own: what follows is still the phone link's TLS, pins and secrets.

Joining. The host opens `wss://alazy.app/relay/v1/<room>?role=host` and waits. The guest opens the same address with `role=guest`; with no host waiting it gets 404 and tries again. A second host, or a guest while one is connected, gets 409. Either end closing closes the other, and the host connects again for the next stream.

Forwarding. The relay passes binary frames from one end to the other as they arrive. Nothing is stored or queued; text frames close the connection.

Encryption. The phone link's pinned TLS runs end to end inside the stream: the guest is the TLS client and the host the TLS server, with the same certificates, pins and device secrets as a direct connection. The relay holds no key and sees only ciphertext.

Pairing through the relay. Pairing by code or link has no QR code to carry the certificate fingerprint, so the match digits and the code check are worked out from both devices' certificates. A relay that answers with its own certificate gives different digits and a different pin, and the pairing fails.

Open: a relay that learns a short code from its room name could try to find a certificate whose digits match (about a million tries). Decide whether code pairing through the relay needs longer digits or is limited to links and QR codes.

What is counted and logged beyond connection times and sizes waits for the owner's answer.
