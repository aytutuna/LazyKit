# Family rules

Rules the Lazy apps share. Each app follows them; `tools/lazycheck.py` checks what a script can check.

## Languages

Every app is localized into every language the App Store offers for listings: 50 localizations, about 45 languages. English (US) is the base text. English (UK, Canada, Australia) are localizations like the others. `testdata/languages.json` has one row per localization with the Apple `.lproj` name, the Android `values-b+` folder, the Windows culture name, the web tag, the right-to-left flag and the wave. The Mexican Spanish row uses `es-419` for the app's own strings.

Wave 1 is the 12 languages that earn most on the App Store: Japanese, German, French, Chinese (Simplified), Korean, Spanish, Italian, Portuguese (Brazil), Russian, Chinese (Traditional), Dutch, Turkish. Wave 2 is the rest. An app may go public before all its languages are in; each language ships in an update once it passes.

Steps for each language in an app:

1. Translate with the app's glossary. `testdata/glossary.json` is the family glossary; an app glossary starts from it and adds its own words. App names and ALAZY INC. are never translated.
2. A second agent reads the language against the glossary and the English text, and fixes or flags what is wrong. Glossary entries stay marked as needing this reader until it has happened.
3. Sweep every screen at phone size and at computer size for cut-off text and, for Arabic, Hebrew and Urdu, right-to-left layout.
4. New texts go into every shipped language before the change reaches main. lazycheck's `languages` rule finds tables that fall behind.

When the first language of an app is done, write down how many tokens it took, in the app's notes. That number times the languages left is the cost of the rest.

Support stays in English only.

## Accent

Inside each app the accent color (buttons, links, selection) is a readable shade of the app's icon color. `tools/contrast.py` finds the closest shade of the icon color that reaches 4.5:1 for text against the surface, once on a light surface (#FFFFFF) and once on a dark one (#1C1C1E). Only the lightness moves. A shade that passes 4.5:1 also passes the 3:1 that controls need; the last two columns show the closest shade for controls alone, for a border or an icon that carries no text.

| App | Icon color | Light: text | Ratio | Dark: text | Ratio | Light: control | Dark: control |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Scan | #2F6FED | #2F6FED | 4.55 | #4780EF | 4.54 | #2F6FED | #2F6FED |
| Print | #0E9F7E | #0C866A | 4.53 | #0E9F7E | 5.09 | #0E9F7E | #0E9F7E |
| PDF | #E5484D | #E12F35 | 4.51 | #E64E53 | 4.52 | #E5484D | #E5484D |
| Send | #7C5CFF | #7958FF | 4.51 | #886BFF | 4.53 | #7C5CFF | #7C5CFF |
| Collect | #E8930C | #A66909 | 4.52 | #E8930C | 6.98 | #D0840B | #E8930C |

The text-grade value is the accent in the app. Run `python tools/contrast.py` to print the table again.

## Relay

Paired devices that cannot reach each other directly talk through the Lazy relay, a Cloudflare Worker at alazy.app/relay/. Devices on the same network connect directly and never touch it.

Rooms. A room is named by the lowercase hex SHA-256 of `lazy-relay-room-v1:` followed by a secret both devices hold: for paired devices the SHA-256 of the device secret (the host keeps only that hash, so both sides can work it out), for pairing by link or QR the 16-byte token, for pairing by short code the 8 canonical characters. The relay only ever sees the room name. Knowing a room name does not let anyone in on its own: what follows is still the phone link's TLS, pins and secrets.

Joining. The host opens `wss://alazy.app/relay/v1/<room>?role=host` and waits. The guest opens the same address with `role=guest`; with no host waiting it gets 404 and tries again. A second host, or a guest while one is connected, gets 409. Either end closing closes the other, and the host connects again for the next stream.

Forwarding. The relay passes binary frames from one end to the other as they arrive. Nothing is stored or queued; text frames close the connection.

Encryption. The phone link's pinned TLS runs end to end inside the stream: the guest is the TLS client and the host the TLS server, with the same certificates, pins and device secrets as a direct connection. The relay holds no key and sees only ciphertext.

Pairing through the relay. Pairing by code or link has no QR code to carry the certificate fingerprint, so the match digits and the code check are worked out from both devices' certificates. A relay that answers with its own certificate gives different digits and a different pin, and the pairing fails.

Open: a relay that learns a short code from its room name could try to find a certificate whose digits match (about a million tries). Decide whether code pairing through the relay needs longer digits or is limited to links and QR codes.

What is counted and logged beyond connection times and sizes waits for the owner's answer.
