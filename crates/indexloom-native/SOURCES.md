# Native adapter source audit

The implementation was written independently in Rust from observed request and
response behavior in local source checkouts under
`/tmp/indexloom-source-audit-20261114`. No C# implementation, upstream test data,
credentials, or account responses were copied into this crate. Rust source and
new fixtures inherit this workspace's AGPL-3.0-or-later license. The observations
below record attribution and interoperability provenance; they do not relicense
upstream code.

Audited revisions:

- [Jackett](https://github.com/Jackett/Jackett),
  `18c313b75a7ddb8b775d717bf9a80c38750051eb` (GPL license; see upstream).
- [Prowlarr](https://github.com/Prowlarr/Prowlarr),
  `d2d1fc0e77664a94b8d3931b7f216eaafd234af4` (GPL license; see upstream).

Paths below are relative to those checkouts and identify exactly what was read.

| Protocol | Source evidence | Independent behavior implemented |
| --- | --- | --- |
| FileList | Jackett `src/Jackett.Common/Indexers/Definitions/FileList.cs`; Prowlarr `src/NzbDrone.Core/Indexers/Definitions/FileList/{FileListRequestGenerator,FileListParser,FileListApi}.cs`; Prowlarr `src/NzbDrone.Core.Test/Files/Indexers/FileList/recentfeed.json` | Basic username:passkey, latest/search action, name/IMDb type, category list, season/episode, integer flags, fixed +02 upload date, 64-bit sizes, torrent URL construction |
| UNIT3D legacy | Prowlarr `src/NzbDrone.Core/Indexers/Definitions/UNIT3D/{Unit3dRequestGenerator,Unit3dParser,Unit3dApi,Unit3dSettings,Unit3dBase}.cs` | api_token query, repeated categories[], imdb/imdbId compatibility, nested data/attributes, boolean freeleech, supplied download/details links and resource-ID/hash distinction |
| UNIT3D modern | Jackett `src/Jackett.Common/Definitions/{aither-api,blutopia-api}.yml` | Bearer auth, perPage, native categories, seasonNumber/episodeNumber, short imdbId, free[]=100, explicit time sorting, percentage freeleech, featured/double_upload/internal, UTC dates |
| AvistaZ | Jackett `src/Jackett.Common/Indexers/Definitions/Abstract/AvistazTracker.cs`; Prowlarr `src/NzbDrone.Core/Indexers/Definitions/Avistaz/{AvistazBase,AvistazRequestGenerator,AvistazParserBase,AvistazApi,AvistazSettings}.cs`; Prowlarr `src/NzbDrone.Core/Indexers/Definitions/AvistaZ.cs`; Prowlarr `src/NzbDrone.Core.Test/Files/Indexers/Avistaz/recentfeed.json` | Form auth, bearer reuse/renewal on 401/412, at least 6.1 s pacing, type/in/limit/page, IMDb, E-only episode variant, native fields, languages, volume multipliers, 404 means empty |

Notable differences resolved deliberately:

- Jackett FileList uses the returned download link; Prowlarr constructs a link
  from configured host/passkey. This adapter uses construction to avoid stale
  hostnames and constrain credential-bearing egress to the configured origin.
- The two UNIT3D source shapes disagree on authorization and freeleech values.
  `Unit3dApi` makes the choice explicit. Numeric IDs are not falsely reported as
  info hashes. Partial discounts preserve their actual volume factor.
- Native category labels are retained because normalization depends on the
  host's definition catalog. This crate does not import thousands of mapping
  constants from C# or assume that framework installations share category IDs.
- Upstream pagination links are data, never executable request targets. Each
  explicit continuation call rebuilds the URL and credentials locally.

## Studied, deferred adapters

RuTracker was examined in Jackett's
`src/Jackett.Common/Indexers/Definitions/RuTracker.cs`, especially encoding,
login/CAPTCHA setup, search and download methods, and Prowlarr's
`src/NzbDrone.Core/Indexers/Definitions/RuTracker.cs` (login, encoding, wildcard
search generation, HTML rows, size/date attributes, and magnet downloading).
Windows-1251 forms/responses, dynamic CAPTCHA fields/images, cookies,
reauthentication, tracker.php row variants and viewtopic/dl.php/magnet downloads
require a distinct browser-session implementation and fixtures. None are
registered here. A FileList API implementation supplies the required real
site-specific adapter without pretending to support that login flow.

Gazelle was examined in Jackett's
`src/Jackett.Common/Indexers/Definitions/Abstract/GazelleTracker.cs` and Prowlarr's
`src/NzbDrone.Core/Indexers/Definitions/Gazelle/{GazelleRequestGenerator,GazelleParser}.cs`.
Browse results can be grouped nested torrents or flat releases; authorization
headers differ between sites; some downloads require cookie/authkey/torrent_pass
combinations. Freeleech-token spending has additional site-specific behavior.
A generic struct with a single assumed auth path would not be complete, so
Gazelle remains unimplemented. UNIT3D supplies the implemented API family.

All adapter fixtures were newly authored from the documented wire shapes:

- `filelist.json`: two artificial releases; Unicode, integer flags, >4 GiB
  sizes and an intentionally retired returned download hostname.
- `unit3d-legacy.json`: hash and numeric IDs, numeric strings, relative/absolute
  signed links, bare UTC and RFC3339 timestamps, explicit next-page link.
- `unit3d-modern.json`: modern percentage discount and internal flags with the
  same independently authored release fields.
- `avistaz.json` / `avistaz-auth.json`: artificial username/password/PID flow,
  redacted bearer/download URL, E-only series example, languages, timezone and
  pagination. Error/expiry responses are authored in the mocked HTTP tests.
- `download.torrent`: generated one-byte (`x`) metainfo with a matching SHA-1
  piece and an inert `.invalid` announce URL; no real tracker passkey.
