# Changelog

All notable changes to the public ElastOS Runtime repository. System shows a
release's Added, Changed, Fixed, Removed and Security bullets before you update;
"Developer detail" is for developers and is not shown.

## [Unreleased]

## [0.8.0-alpha.15]

Test-channel release. Signed releases show these notes in System before you update.

### Added

- You can remove ElastOS with the installer:
  `curl -fsSL https://elastos.elacitylabs.com/install.sh | bash -s -- --uninstall`.
  It stops ElastOS and removes the program and its apps but keeps your identity
  and files, so installing again continues where you were. Add `--remove-data`
  to delete those too; keep your Recovery Kit first.
- An owner who lost every passkey can recover their Home with their
  Recovery Kit, even when guest accounts are off. Before, the sign-in screen
  offered no way back. Choose "Recover this Home with your Recovery Kit" on the
  sign-in screen, run the command it shows on the computer that runs Home
  (`elastos identity arm-owner-recovery --origin … --rp-id …` with that page's
  address filled in), then enter the code the command prints.
- Browser installs and runs on Apple Silicon Macs and on Linux ARM64 computers
  such as a Jetson. On Linux it needs a one-time host setup, which the
  installer offers.

### Changed

- Browser is hidden on computers without a local Engine (anything other than
  an Apple Silicon Mac or a Linux ARM64 computer such as a Jetson) for now.
- On Linux, Browser asks you once to run its one-time host setup again after
  this update. Run the command it shows; Browser then works as before.
- Setup installs one Home; it no longer prints options for other setups.
- The terminal Home no longer offers the old native Chat, which used a
  different network from the Chat on the Desktop.
- People "Message" in the terminal Home no longer opens that old Chat. In the
  browser terminal it opens Chat on the Desktop at that person's conversation;
  in a native terminal it says "Open Chat on the Desktop to message <name>."
- `elastos --help` lists only the everyday commands. Commands for people who
  run servers or publish releases (such as `serve`, `gateway`, `keys` and
  `publish-release`) still work and still have their own `--help`.

### Fixed

- On Linux, Carrier sends one UDP datagram at a time. Some Wi-Fi drivers, such
  as the Jetson's, dropped batched datagrams, which stalled connections, Chat
  and downloads.
- The terminal Home no longer quits with "403 Forbidden" after the first
  action; you can run actions one after another.
- In the terminal Inbox, direct messages and service requests say "Open Inbox
  on the Desktop to answer this" instead of "no longer available".
- In the terminal Apps tab, Enter on an app that only opens on the Desktop
  says "Open <App> on the Desktop" instead of doing nothing.
- Publishing a document (Documents Publish and `elastos share`) works again:
  it copied a viewer file from a place the Documents app does not ship (#128).
- Importing your Recovery Kit with a new passkey now keeps your admin role.
  Before, an owner who added a guest passkey and imported their Kit stayed a
  guest, and the Home was left with no admin.
- Home no longer runs out of memory when it starts with Qwen2.5 1.5B on the
  device. On an 8 GB Linux computer, Home used more than 7 GB within seconds of
  starting, and the system stopped it.
- On Linux, a reply right after Get no longer fails with "Not enough free
  memory" when most of the memory in use is only cached files.
- `elastos setup` on a Home that imported media tools before 0.8.0-alpha.9
  stopped with "supplied media tools differ from the private imported pair".
  On an installed signed release, setup now fetches and verifies the release's
  media tools again and replaces the private copy, keeping your media limits.
- While a model downloads, Marketplace and System no longer stop at "Status is
  unavailable. Refresh to check before trying again." after one missed status
  check. They show "Checking status…" and keep checking.
- While a model is being set up after its download, Marketplace and System show
  "Finishing setup…" instead of "Model service unavailable." with a Retry button.
  If setup takes more than two minutes, Retry comes back.
- When no model is ready yet and one is downloading, Assistant shows, for
  example, "SmolLM2 is downloading (42%)" instead of "No model".
- System > Shell and Marketplace > Home views no longer offer Terminal unless
  developer mode is on. If Terminal was chosen and can no longer start, Home
  opens the Desktop instead of showing an error after every reload.
- A new Home no longer asks in Inbox for market-price access before you open
  Wallet.
- Assistant no longer shows a microphone button that cannot work inside Home.
- The welcome sheet no longer talks about an "updated" Recovery Kit on a new
  Home, and the lock screen shows the first letter of your name.
- Apps that ask for the ElastOS version now get the same version that
  `elastos --version` and System show.
- A model download that stopped with "Model content could not be read" now
  continues from where it stopped when you press Retry. Before, Retry started
  the download again from 0%.
- Right after Get, Assistant's model menu no longer lists the new model by its
  file name, such as "qwen2.5-1.5b-instruct-q4-k-m-local". It shows the model's
  name from the model list, such as "Qwen2.5 1.5B Instruct", as soon as Home
  has finished setting it up, without pressing Refresh models.
- When the installed version record is damaged, `elastos update` now tells you
  to run `elastos update --force` even if it cannot reach the update source.
  Before, a connection error hid that advice.
- When an update is already running, started from a second System window,
  System now says "Another update was started on this Home. Wait for it to
  finish, then check the update again." When a later approval has replaced
  yours and nothing is running, it says "This approval is no longer valid.
  Check for the update again and approve it once more." Before, both said the
  release was refused or verification failed.
- If the connection drops while `curl … | bash` is still sending the
  installer, nothing happens: no Runtime is stopped and no file changes. Before,
  bash ran the part that had arrived, which could leave the Runtime stopped.
- Runtime setup and the first Home started by the installer keep your own
  temporary folder. Before, they got the installer's temporary folder, which it
  deleted when it finished.
- The installer downloads release data over HTTPS only and stops with a clear
  message if a server redirects it to plain HTTP.
- Running the installer again while Home is open now works: it closes Home,
  installs, and opens Home again. Before, it stopped with "Recorded Runtime is
  foreign or ambiguous".
- On a Mac without Apple's Command Line Tools, the installer now stops before
  downloading anything and says: run `xcode-select --install`, then run the
  installer again. Before, it failed with "Invalid release-head.json format".
- The installer no longer prints a raw JSON receipt near the end; it prints
  "Protected roots are ready". A receipt for moved data stays in its backup
  folder.
- When setup or an update cannot reach the trusted source, ElastOS now says
  so in plain words: it needs outgoing UDP to port 4433, so check the firewall
  or network, then run `elastos setup` again.
- With no model yet, Assistant now says "Get a model to start. Models run on
  this Home." and shows a Get a model button that opens Models in Apps. Send
  says the same when you point at it.
- Assistant's other no-model message now names the real app: Apps
  (Marketplace) → Models.
- Pressing Stop in Assistant now shows "Stopped." instead of "Settlement
  unknown".
- If there is not enough free disk space to get a model, Apps now says so and
  how much space the model needs, instead of "Model action could not be
  confirmed". While a model waits for space, it shows how much it needs.
- Assistant's description in Apps now reads "Chat with an AI model that runs
  on this Home."

### Removed

- The sign-in screen no longer shows "Use an older security key". To replace a
  passkey, restore your Recovery Kit and create a new one.
- Protected content is not part of 0.8.0. Elacity Player is no longer listed in
  Apps or search, Library no longer offers "Protect and List…", and Marketplace
  no longer has a Media tab. They return in a later release.
- `elastos agent`. Its app was retired earlier and the command could not work.
- `elastos upgrade`; use `elastos update`. The `--no-confirm` spelling of
  `--yes` on `elastos update` is gone too.
- `elastos update --gateway` and `--no-p2p`, and the installer's IPFS-gateway
  options (`--gateway`, `--head-cid`, `--publisher-node-id`). Updates come
  only from your trusted source over Carrier; the installer downloads only
  from its publisher address. `elastos update --head-cid` and `--rollback-to`
  stay.
- ElastOS no longer attaches downloads to GitHub Releases. Install with
  `install.sh`; it and System fetch the signed release over Carrier.

### Security

- An installed Home runs a local capsule only when it comes from the signed
  release or is signed by the release publisher. Homes built from source keep
  running unsigned capsules. This needs no action after the update.
- Recovery Kit import accepts only a Kit made on this Home for that account,
  and refuses any import that would leave the Home without an admin passkey.
- Requests and replies from other computers now have limits and deadlines:
  whole-object fetch replies are limited to two at once and one per peer, and
  a connection passing data through your computer closes after five minutes
  with no data in either direction.

### Developer detail

- The local pre-push gate now runs the CI behaviour suite and the CI
  source-gate checks before Cargo work. CI, `just test-behaviour` and the gate
  share `scripts/test-behaviour.sh`; CI source-gate, `just verify` and the gate
  share `scripts/test-source-gate.sh`. A failing check stops the push locally
  and names the test. Run `just test-behaviour-inputs` once to install the
  pinned Playwright inputs.
- Removed `elastos identity passkey-hint` and the private credential-hint
  sign-in path. Anonymous sign-in options still never list an account's
  credentials.
- The capsule catalogue marks `home-cli` not launchable unless the host
  terminal policy allows it (`developer_mode = true` and closed guest
  registration), so Home view candidates, System and Marketplace follow the same
  rule; the active-shell summary already repairs a saved shell that is no longer
  a candidate. Home host also resets a non-launchable saved shell to `home-gui`
  through `POST /api/apps/home/active-shell`
  (`scripts/home-shell-unavailable-shell-fallback-smoke.mjs`).
- `protected_content = true` in the host `config.toml` (`elastos config set
  protected_content true`; default off) is the one switch for protected content:
  off, the catalogue drops `elacity-player` and reports
  `policy.protected_content: false` (Marketplace hides Media), Library objects
  lack the `protect` capability that "Protect and List…" requires, and
  `object/publish` with `protection` is refused.
- `GET /api/wallet/prices` raises the CoinGecko Inbox request only with
  `request_access=true`, and not within 24 hours of a dismissal or rejection
  (new `dismissed_at` on Inbox entries). Wallet sends it unless it was preloaded
  by the rail (`preload=1`); opening the rail sends `elastos:wallet-refresh`,
  which asks.
- `model-management.js` adds the `finishing` phase (admitted, activation
  pending, no dispatch-unavailable reason), which turns back into the
  unavailable phase with Retry after 120 s. `model-selection.js` adds
  `downloadPercent` and `downloadingModel`.
- `components.json` keeps only the `home` profile and the `browser-host`
  helper set that setup installs automatically; `demo`, `operator`,
  `blockchain`, `agent-local-ai`, `public-gateway`, `minimal` and `full` are
  removed. Developer components (GBA, site, tunnel, cloudflared, availability
  and the DRM, rights, key, decrypt and custody providers) still publish with
  each release and install with `elastos setup --with <name>`.
- `--profile`, `--with` and `--without` are hidden from `elastos setup --help`
  but still accepted; the help no longer names a nonexistent `chat` profile.
- MyWebSite and Shared leave the home-cli for 0.8.0; `elastos site`, `share`
  and `shares` stay operator commands. The Rust `demo` publish profile is gone;
  `scripts/home-demo-local.sh` runs plain `setup`, and the operator smoke and
  pinned-model proof use `--with`.
- `elastos update` and System Update stage this profile change normally; only
  the standalone frozen offline path refuses a `profiles` difference.
- Home consumes `intent.json` by overwriting it with empty content instead of
  deleting it; the session holds only read and write capabilities. A
  regression test runs two intents against a provider that refuses delete.
- The `chat` core action and `ManagedChat` launch are removed from Home.
  `elastos chat` is hidden from help and kept only for the native chat smoke
  scripts. `people-message:` emits a `switch-shell-open-target` host intent for
  `chat-room` with `{conversation_id}` (the query People sends); the gateway
  accepts only that bounded query for People hand-offs. Home and home-cli key
  messaging on the contact's `conversation_id` (the People summary never had
  `route`), and `run_chat_from_home` is removed.
- `elastos agent`, `agent_cmd.rs` and the shell's `agent` command request are
  removed. The `capsule` help, the serve banner, the first-run welcome and the
  microVM publish hint (no `serve --forward` flag) name commands that exist.
  `docs/COMMAND_MATRIX.md` drops `chat`, `agent` and seven nonexistent `room`
  subcommands; GETTING_STARTED no longer suggests `elastos chat`.
- Share bundles copy the viewer from its manifest `entrypoint`
  (`browser/index.html` plus its assets) to the bundle root. The entrypoint
  must be a relative path inside the viewer. Every path component is opened
  with `openat(O_NOFOLLOW)` from the viewer directory's descriptor, so a
  symlink anywhere in the copied tree, even one swapped in after listing, is
  refused; the copy is capped at 256 files and 16 MiB. The Documents
  publish test installs the real `capsules/documents` layout and checks the
  published file list.
- Startup restore and repair of complete local Content objects skip
  Runtime-private `model-preparation`. Admitted models are verified at
  admission, and this Home's Kubo does not hold them in 0.8.0; serving them to
  peers is the holder role (#324). Before this, restore read each admitted
  model file whole, then base64-encoded it and copied it through the provider
  bridge, using about 8 times the file size.
- Restore and repair now stream every local object from disk into Kubo,
  unpinned, through a new Runtime-private ipfs-provider operation
  (`runtime_add_local_object`). It uses the same parts and add options as
  `add_directory`, so the CID matches the original import. The provider opens
  the object below its data root and each file without following links
  (regular files owned by this user only). Content pins only a matching CID:
  repair pins the requested CID, and restore pins a CID with a Content receipt
  that was not withdrawn by unpublish. Runtime hashes the files with a streamed
  reader and sends only their paths and sizes, so memory stays the same
  whatever the object's size.
- model-provider's cgroup v2 memory check subtracts reclaimable file cache
  (`active_file + inactive_file - file_dirty - file_writeback` in
  `memory.stat`, never below zero) from `memory.current`.
- `elastos update` checks the installed version before connecting to Carrier
  (#156). Runtime admission and the release integrity checker now read one
  platform table, `scripts/release-platforms.json` (`darwin-arm64` =
  `aarch64-darwin`, `darwin-amd64` = `x86_64-darwin`). The checker no longer
  uses `macos-arm64`, so it no longer skips the Mac entries Runtime admits.
- Capsule admission (`Runtime::configure_capsule_admission`): a Home with an
  installed signed release verifies each capsule against the signed release:
  it trusts the default trusted source's publisher DID keys plus
  `trusted_keys`, and admits a capsule without a publisher signature only from
  its release install path when `components.json` matches the signed release
  and the capsule's `.elastos-cid` and archive receipts match its entry.
  Source and CI Homes keep developer mode explicitly (`setup-source-home.sh`,
  `ci-installed-journeys.py`). The new Runtime applies this on its first start
  and leaves `config.toml` unchanged.
- `commit_recovered_root_reassignment` sets the importer's role inside the
  same auth-state mutation: Admin if the importer or any replaced record of the
  recovered principal or root was Admin, otherwise Guest. The commit fails, with
  auth state unchanged, if no active admin passkey would remain. (#88)
  Reassignment also requires that, when this Home already knows the recovered
  principal or root, a stored `principal_root_protections` entry for that exact
  principal/root has the Kit's `data_key_id` (the Kit unwraps the same data
  key). The import route checks this before deriving keys, activating
  protection or creating the recovery archive key, so a refusal writes only
  the denial audit event. The Kit's archive is sealed inside the commit's auth
  lock; if this commit created the recovery archive key and the auth file on
  disk does not reference the new archive after a failure, the key is removed.
- New `elastos identity arm-owner-recovery --origin --rp-id [--expires-in ≤900]`
  stores the digest of a one-time code (`AuthState.owner_recovery`); the Home
  must already have an admin. `POST /api/auth/owner-recovery/begin` and
  `/complete` (header `x-elastos-owner-recovery`) register a passkey only when,
  in one auth write, the code matches (constant-time), has not expired, and the
  presented Kit belongs to an existing Admin record whose stored root protection
  has the Kit's `data_key_id`. That write replaces the admin's records, revokes
  their sessions, binds the new passkey as Admin, consumes the code and audits
  `auth.owner.recovered`. Both routes share the passkey-registration rate
  limit, and each checks the code (armed, unexpired, constant-time match)
  before any password derivation or Kit work; an unarmed or expired Home
  refuses without writing, and the fifth wrong code burns the armed code. A Kit
  refusal leaves auth state and the credential store unchanged. The credential
  is written just before the auth write; a crash between them leaves an orphan
  credential that a retry with the same passkey reconciles. (#88)
- Assistant empty state: `showEmptyState()` builds its lines with DOM nodes;
  `paintEmptyModelHint()` shows the no-model line and a `data-agent-get-model`
  button (`openModelsFromAgent()`, Marketplace `category: models`) only after a
  successful offers read returned none (`endpointState: "no-model-offers"`) and
  no model is downloading. A failed read or an unavailable chosen model beside
  others keeps the usual hero and model menu. The harness refreshes it on
  `assistant:live-models` via `refreshEmptyModelHint()`. `syncAgentSendButton`
  titles a disabled Send "Get a model to start".
- `streamChatViaContract` results carry `stopRequested`. A user Stop that the
  engine settles as `settlement_unknown` shows `STOPPED_STATUS`; the turn
  manifest keeps `settlement_unknown`.
- Preparation `use` refused by `require_space` now reports the model's whole
  unwritten size (`NotEnoughFreeSpace { needed: unwritten }`), not the zero-byte
  probe. The capsule invoke gateway maps it to HTTP 507
  `not_enough_free_space` with a plain message; `_shared/model-management.js`
  shows that message, leaves Get available and skips reconcile. The
  `capacity_pending` phase text is built by `capacityText()` with the remaining
  bytes.
- `Commands` in `main.rs` marks `serve`, `publish-release`, `content`,
  `collaboration-config`, `protected-content-config`, `node`, `gateway`,
  `sign-payload`, `keys`, `tls` and `emergency` with `hide = true`;
  `docs/COMMAND_MATRIX.md` lists them. Removed the `Upgrade` variant, the
  `no-confirm` alias, and the retired hidden `publish-release --key` /
  `--allow-no-bootstrap` flags with their `PublishReleaseOptions` fields. The
  `just publish*` recipes, which always failed on the retired `--key`, are gone.
- `get_runtime_info` in `resource_bridge.rs` (both the local and the attached
  bridge) returns `ELASTOS_VERSION` from `build.rs` instead of
  `CARGO_PKG_VERSION`.
- `carrier.rs` reads peer lines through `read_peer_line` (`take(max + 1)`
  under a timeout). Caps come from the protocol limits: `handle_file_stream`
  takes the I/O bridge line limit or one `GOSSIP_WIRE_MESSAGE_MAX_BYTES`
  message plus 64 KiB of envelope, within 10 s; `gossip_pull` replies take
  `GOSSIP_TOPIC_BUFFER_MAX_BYTES` plus envelope; open-ended provider replies
  take one `MAX_CARRIER_OBJECT_IMPORT_BYTES` object as base64 within an hour
  and share a 256 MiB in-flight budget (`CARRIER_REPLY_BUDGET`; refused past
  it); whole-object content fetch replies keep no size cap (the object is one
  JSON line) but run two at once and one per peer;
  bounded reads keep 128 KiB and now also get the route deadline on peer-DID
  routes. `operator_control.rs` reads requests (before signature checks) at
  64 KiB / 10 s and replies at 1 MiB / 1 h. Relays in `carrier.rs`,
  `carrier_engine_media.rs` and `carrier_engine_binding.rs` close after 5 min
  with no bytes either way (`RelayActivity`), and stream acks have deadlines.
- Model preparation keeps its stage after any `MetadataRead`, `WeightsRead` or
  `Capacity` failure, not only after a read typed as in transit (`Io`). A part
  that a holder refuses or answers malformed is rejected before it is written,
  so the staged prefix stays valid; the next Use re-hashes it and continues
  after the last whole part. Integrity, local storage, authority, policy and
  Cancel still remove the stage. Seen on a Jetson running alpha.14: a read that
  ended as a `Provider` error at 78.7% sent the next Retry back to 0%.
- Assistant lists a Runtime-admitted model offer (`model:<sha256>`) only through
  its catalogue row (title and CID); `liveMenuOffers()` leaves it out of the raw
  offer rows, which could not be chosen anyway. While Assistant is open
  (`watchLiveModels`), `agent-live.js` reads the catalogue again every 3 s when
  such an offer is listed but its catalogue row is not yet bound to it
  (`dispatch_ready: false`, for example the first read after activation), at
  most 10 times, and while a model downloads with nothing selectable. One timer
  serves both, it skips while a read is in flight, and closing Assistant stops
  it. Every probe result is published as `assistant:live-models`; the harness
  updates the chip and an open menu from it, keeping keyboard focus on a row or
  menu action. Before, an open menu kept the earlier read while the chip changed.
- Owner update refusals caused by another owner action are typed. The System
  update route answers `update_controller::UpdateInProgress` (owner-action lock
  held, `WouldBlock` only, or a different request queued or active) with 423,
  and `ApprovalReplaced` (a later approval replaced the receipt and nothing is
  queued or active) with 410; `system.js` shows the plain messages above. Other
  lock I/O failures and refusals keep 409. Covered by
  `system_update_refuses_a_concurrent_tab_and_a_replaced_approval`.
- `scripts/install.sh` runs every action inside `main`, called on its last
  line, so bash parses the whole script before acting. `install-bootstrap-test.py`
  feeds the installer truncated at each step and after the Runtime stop, and
  asserts no request, output, Runtime call or HOME change.
- `install.sh` keeps its scratch directory in `INSTALL_SCRATCH` and never
  assigns `TMPDIR`, so `elastos setup`, Home and the update controller's saved
  launch environment see the caller's `TMPDIR`. Tests: a policy check, and the
  fixture's `setup` stub records the `TMPDIR` it receives.
- `install.sh` fetches through `fetch_url`, which runs curl with
  `--proto '=https' --proto-redir '=https' --tlsv1.2`. Only an exact
  `http://127.0.0.1`, `http://[::1]` or `http://localhost` origin (optional
  port, no userinfo) may use HTTP, and such a fetch follows no redirect
  (`--max-redirs 0`). curl's exit 1 (protocol refused) names the URL. The
  bootstrap fixture's curl stand-in requires these flags on every request;
  real-curl tests cover userinfo tricks and HTTP and HTTPS redirects to plain HTTP.
- Removed the web transport override from the Runtime update flow (gateway
  head discovery, `/ipfs/<cid>` fetches, the gateway release-manifest path and
  the third-party IPFS host list) and the installer's CID-gateway bootstrap
  (`ipfs_fetch`, `ELASTOS_IPFS_GATEWAYS`) with its docs and fixture transport.
  Dead installer values (`PUBLISHER_HASH`, `LAST_SUCCESS_GATEWAY`) are gone.
- The installer's identity-checked stop recognises this installation's Home:
  `<bin> home …` is a Runtime command, and when its parent is this data
  root's `update-controller/runtime __update-controller --receipt …`, the
  controller is stopped first so it does not restart Home. Real-process fixture
  test: `test_open_home_stops_through_its_update_controller`.
- `install.sh` still needs python3 to check the Ed25519 signature: stock macOS
  `openssl` (LibreSSL 3.3.6) has no Ed25519. Its preflight now runs
  `python3 -c ''` and, if the macOS stub (or any broken python3) cannot run,
  stops with one next step before the first request. Test:
  `test_python3_that_cannot_run_stops_before_any_download_with_one_next_step`.
- `install.sh` discards `principal-root-upgrade`'s stdout receipt (written to
  the backup directory when objects move) and prints one line. The command's
  JSON output is unchanged for the source-restart scripts that parse it.
- Setup, Home capsule launch, update refresh and the supervisor install every
  component only over Carrier by its signed `release_path`; the HTTP
  `{gateway}/ipfs/<cid>` content path (`download_streaming`,
  `build_gateway_list`, `ElastosFetchPath`, the update HTTP download helpers)
  is removed. Admission refuses a component without a non-blank `release_path`
  (`component_without_signed_release_path_is_refused_before_fetch`). sources.json
  `gateways` now serves only Carrier ticket refresh.
- `CarrierClient::connect_trusted_source` wraps a failed connection in that
  plain message; the per-route ticket and fallback errors stay in the cause
  chain (`{:#}`). Test: `unreachable_trusted_source_names_the_network_fix`.
- `install.sh --uninstall [--remove-data]` (#315) first refuses, before
  stopping anything, a data directory that is a link, belongs to another user,
  is not named `elastos` or is `$HOME` or one of its parents (compared by device
  and inode, so aliases and case variants count), and a release entry that is a
  link. It then stops the installation with the installer's identity-checked
  `installer_runtime_control`, takes the Runtime writer's
  `.elastos.install.lock` and refuses while it is held or an update journal
  exists. It removes the binary and the release-owned data entries (`bin`,
  `capsules`, `libexec`, `tools`, `model-catalog.json`, `components.json`,
  `sources.json`, `installation`, `update-controller`); with `--remove-data` it
  removes the whole data directory instead, without following links. It
  downloads nothing and needs no trust anchors. `docs/INSTALL.md` now lists the
  macOS and Linux paths and documents removal.
- `docs/VERSIONING.md` "Withdraw a bad release" (#321): publish N+2 from the
  last good source, announce on #93, point affected Homes to Undo; the seed
  keeps N pinned. Decision: no withdrawn-versions list in the signed head for
  0.8.0; revisit with #157 and #213.
- The local pre-push gate refuses what CI refuses: its Cargo calls use
  `RUSTFLAGS=-D warnings` and drop `CARGO_ENCODED_RUSTFLAGS`, clippy covers
  the touched packages and their reverse dependencies in each workspace, and a
  change to `scripts/release-*`, `scripts/install.sh` or
  `scripts/publish-release.sh` runs `release-platform-input-test.py` and
  `ci-release-policy-test.py`. Documentation-only changes keep fmt and git
  checks without rebuilding unrelated workspaces (#319). The installed Home
  journey waits the model-preparation window for the first engine refusal.
- The `custody-harness-smoke` path filter and the Linux holder canary's paths
  now include every input their jobs build or call (setup and restart scripts,
  `chain-provider`, `elastos-runtime`, the workspace manifest and lock,
  `rust-toolchain.toml`, the Cargo configuration it reads, `components.json`,
  the workflow, the canary's `localhost-provider` capsule, hop observer and
  Kubo seed scripts, and the crates each capsule names by path).
  `ci-release-policy-test.py` follows each job's shell scripts, Dockerfiles,
  Cargo configuration, `-p` capsule packages and path dependencies, and fails
  when a called input is outside the filter (#317).
- `ci.yml` no longer publishes a GitHub Release or uploads `elastos-runtime-*`
  packages on tags, `main` or dispatch: those unsigned assets were a second
  channel beside the signed Carrier release. CI still packages ARM64 for the
  Jetson compatibility gate. The nine required checks are unchanged.
- New `update-hop-linux` CI job proves install and update on
  Linux x86-64 as `source-home-macos` does on Mac: the pinned published release
  (0.8.0-alpha.8, which carries x86_64-linux artifacts) installs with its own
  `install.sh`, its System installs this source and Home reconnects, this
  source's System refuses a tampered offer and applies the next release, the
  CLI refusals (wrong signers, platform, version, tampered binary) leave Homes
  unchanged, and `update --rollback-to` undoes the hop. `update-hop-compare.py`
  runs CLI fixtures natively on Linux (`HOME/.local/share/elastos`, ELF
  admission, `xdg-open` fixture opener, Runtime's platform aliases). The seed's
  HTTP route does not serve every Linux artifact, so its new
  `install-previous-release` installs a disposable Home with the published
  `install.sh`, rolls it back over Carrier to the pinned head (`head_cid`, new
  in `update-hop-previous-release.json`) and copies the release out under its
  signed hashes, rebuilding capsule archives as releases build them. On Linux
  and Mac, the `check-result` verdict in the job summary also requires and
  names the next release, the refused tampered offer and each CLI refusal (#320).
- Linux Browser's prepared rootfs pool names each copy by the admitted rootfs
  hash from the image manifest. Launch boots only copies of the current image
  and removes the rest, so an image update or Undo never boots the previous
  rootfs with the new kernel; a refill that sees the image change discards its
  copy (#307).
- `elastos setup` on Linux writes a per-Home TURN credential to the owner-only
  `runtime-turn/turn-credentials.env` (kept on later runs), so a fresh
  Linux/Jetson Home gets the session-local TURN configuration the crosvm
  launcher requires (#307).
- Browser TURN relays only to its own relay address on both hosts: coturn
  denies loopback, private ranges and every other peer. On Mac, a loopback relay
  keeps loopback peers, and the media bridge closes any guest TURN stream that
  addresses a peer outside the session relay ports or asks for a TCP relay
  (#307).
- Linux Browser network setup adds a per-slot output rule: UDP from the TURN
  relay ports reaches only the slot's TURN ports. coturn's RFC 6062 TCP
  relays are off on both hosts. `check` refuses a setup receipt without the
  rule, so Browser asks to re-run the root setup once (#307).

## [0.8.0-alpha.14]

Test-channel release. Signed releases show these notes in System before you update.

### Added

- Marketplace offers a second local model, Qwen2.5 1.5B Instruct (1.1 GB,
  Apache-2.0), for noticeably better answers than SmolLM2. It needs about 3 GB
  of free memory.

### Changed

- This release updates the model list. If SmolLM2 is already on this device,
  press Use once; it keeps the downloaded files.
- If this Home is on 0.8.0-alpha.12 or older, Update here stops with "The update
  could not start" and keeps your current release. Run the installer again
  instead (`curl -fsSL https://elastos.elacitylabs.com/install.sh | bash`); it
  keeps your identity and data.

### Developer detail

- Signed model catalogue 3 is pinned in `components.json` (head
  `bafkreid5lc25ap4v3qw576uw3z3zdsbsfhqnnt5uwfdkawju4ejrtin7z4`): SmolLM2-135M
  unchanged plus Qwen2.5-1.5B-Instruct Q4_K_M (package
  `bafybeievyi6xp5b62qkuh65ywkd72medhx6uxrmik2fhnr3aameeqnxvwe`, reproducible,
  pinned on the seed). alpha.13's updaters install the new catalogue; alpha.12
  and older System updaters cannot (#84, #113).

## [0.8.0-alpha.13]

Test-channel release. Signed releases show these notes in System before you update.

If your Home is on 0.8.0-alpha.10 or older, update it once from Terminal with
`elastos update`; the System page in those releases cannot install this one.

### Changed

- A model download that stops because the network failed, or because Home
  restarted, now continues from where it stopped the next time you press Use,
  instead of starting over. Cancel on the stopped download discards the part
  already downloaded; Home also discards it after 7 days, or when the model is no
  longer offered.
- A local model now starts only when the device has enough free memory for it.
  Otherwise you see "Not enough free memory for this model. Close other apps or
  choose a smaller model." instead of a model failure.
- A local model stops after a minute without use to give its memory back, and
  starts again on your next message. Only one local model runs at a time.
- When the model list is updated, a model already on this device says "Model
  list updated. Press Use to keep using this model." Use keeps the files; nothing
  downloads again.
- Marketplace shows a model's own description from the signed model list.

### Fixed

- An update made by an older version could leave out the release's signed model
  list, after which Home, Undo and later updates stopped with "Installed release
  inputs require repair". Home, Update and Undo now fetch the release's own
  signed model list from your trusted source and continue. Without a connection,
  Home still starts and Local AI says the list could not be fetched; Update and
  Undo say so and change nothing.
- Update and Undo now carry the signed model list with the release's other
  files, in Terminal and in System, so a release with a new model list installs
  cleanly and Undo puts back the previous list.
- An Undo that stopped halfway could leave one component from the other release,
  after which Update, Undo and `elastos setup` refused to continue. Home, Update
  and Undo now put back this release's signed copy, and `elastos update` does so
  even when no newer release exists. Replacing a component folder no longer
  removes the old one first.
- `elastos setup` fetched the newest release's components by name, so setting up
  an older release failed. It now fetches that release's own files by their
  signed IDs.
- A brief network stall no longer stops a model download. Home tries a part
  that timed out up to three more times before it stops.
- After about 64 model downloads, retries or cancels, Home could no longer get
  any model. Old finished attempts no longer use up that limit.
- Setting up Home on an ARM64 device without the CPU features local AI needs no
  longer fails. Home installs without the local model engine, and Marketplace
  shows "Local AI is not supported on this device."

### Developer detail

- Home start, `elastos update`, System update and Undo first repair the installed
  release from its signed components: a missing or wrong-head pinned model
  catalogue is fetched by CID over Carrier and checked for CID and publisher
  signature; a component with the wrong bytes is re-fetched by CID. Both update
  paths install the target's catalogue as journaled support. Setup fetches
  components and the catalogue by the release's signed CID instead of by name.
  The catalogue pin stays at alpha.11's head
  (`bafkreihmcsvldwwa5ttbgw5yfrs62kiv3natqhfj3tkg3yzemsd73runr4`) so alpha.11 and
  alpha.12 updaters, which do not install catalogues, can update to this release
  (#113).
- `model-provider` admits a local engine right before spawn when free memory
  covers GGUF weights once, the f16 KV cache at the configured context, batch
  scratch and headroom of max(10% RAM, 1 GiB). Free memory is Linux
  `MemAvailable` capped by every cgroup v2 ancestor's `memory.max -
  memory.current` (cgroup v1: `MemAvailable` only), or macOS free (minus
  speculative) + purgeable + file-backed pages. When that estimate is short,
  macOS still admits if `kern.memorystatus_vm_pressure_level` is normal and the
  need is at most 40% of RAM. Refusal is `model_memory_unavailable` (#98).
- Local runs share one execution slot; an engine idle for 60 s is stopped and
  opening another model closes the previous engine. llama-server gets
  `--batch-size 128 --ubatch-size 128 --cache-ram 0`. `elastos setup` drops
  `llama-server` on an `UnsupportedHost` instead of aborting.
- A model part read that every holder failed only in transit (answer deadline,
  connect failure, lost stream; `carrier_fetch_in_transit`, mapped to
  `ProviderError::Io`) is repeated up to 3 times after 0.5, 1 and 2 s in the same
  in-flight slot, after the same per-part checks as a first read. Refusals,
  malformed or oversized answers and integrity, authority or capacity failures
  are never repeated. The seed's `ipfs-provider` bridge serializes requests, so
  one slow read there runs every queued read's 5 s budget out together.
- Download resume: `settle_failure` keeps the stage (`kept_stage`) after an
  in-transit read failure, a stopping Home, a passed deadline or restart
  reconciliation, and removes it on cancel and on any refusal, integrity,
  authority, policy or storage failure. The next Use adopts it only if its
  `_elastos_object.json` equals the signed `object_manifest`, re-hashes complete
  files, truncates a partial file to a 64 KiB boundary and fetches only the
  remaining parts; per-file SHA-256 and package CID checks are unchanged. A kept
  stage stays charged against the cache budget until adopted, cancelled, or
  removed at startup after 7 days or when no longer offered.
- The 64-record preparation cap bounds live work only: `reserve_at` prunes
  ownerless Failed, Expired or Cancelled records settled at least an hour ago
  (`settled_at`) before refusing; expiry is saved before any refusal.
- `ci-model-package.mjs produce FIXTURE OUT DID KUBO_DATA` builds one package from
  a pinned, validated fixture; `catalog CAT DID ENTRY...` writes an unsigned
  payload only after `release-signer.py`'s own catalogue check accepts it.
- The model projection reports `dispatch_unavailable_reason: "catalog_updated"`
  only for an admission bound to an earlier head whose package the current
  trusted catalogue still offers under a current local-use grant.

## [0.8.0-alpha.12]

Test-channel release. Signed releases show these notes in System before you update.

### Changed

- Models download several times faster: Home now fetches up to eight parts at
  once instead of one after another.

### Fixed

- Getting a model on a slower connection no longer stops after about ten minutes
  with "Storage capacity is unavailable". Home was shutting down its storage
  service as idle while the model was still downloading.

### Developer detail

- Model preparation keeps up to 8 part reads in flight across the package; each
  part's authority and capacity checks run in package order just before its read,
  results arriving after a revocation are discarded, and per-file SHA-256 and the
  package CID check are unchanged. Real installs spent ~200-600 ms per part on
  round trips while the seed's own read takes ~2.5 ms (#84).
- `ipfs-provider` counts a successful private capacity check (run once per MiB of
  model download) and staged-directory hash as Kubo use, refreshing
  `ipfs-coords.json` `last_used` like bounded reads do, so the 600 s idle stop no
  longer ends Kubo during a long model download (#97).

## [0.8.0-alpha.11]

Test-channel release. Signed releases show these notes in System before you update.

### Added

- Marketplace offers a local AI model, SmolLM2 135M (145 MB, Apache-2.0). Press
  Get and Assistant can answer with it on this device; the AI engine downloads
  the first time a model needs it. Answers from this small model are basic; larger
  models follow.

### Changed

- Models download several times faster: after the first piece, a Home reads the
  rest straight from the Homes that hold it.

### Developer detail

- The first production model catalogue is signed offline with the maintainer key
  (`release-signer.py --model-catalog`, policy field `model_catalog_sha256`) and
  pinned in `components.json` (`model_catalog`, head
  `bafkreihmcsvldwwa5ttbgw5yfrs62kiv3natqhfj3tkg3yzemsd73runr4`); the package
  `bafybeiew3vuq32fvuz2kmps7lmgl4rxpvwsvklxykx4covogbrkhgy5qky` is built by
  `ci-model-package.mjs produce` and pinned on the seed (#84).
- CI proves Marketplace Get and an Assistant reply on Mac, Linux x86-64 and
  ARM64; on x86-64 the package lives only on a separate holder Home, so Get
  crosses Carrier (#150).
- Model preparation tries the local store only until its first miss, then reads
  remaining parts from Carrier holders with unchanged per-file SHA-256 and
  package CID checks (145 MB: 332 s to 42 s locally).
- CI retries apt with backoff and cancels a pull-request run at its first failed
  job (#194).
- `publish-release` no longer sends a release gossip announcement that no Home
  received. Seeds on an older Runtime still print the old announcement error after
  the commit (#174). INSTALL.md describes how updates reach a Home, and
  VERSIONING.md describes the seed's role.

## [0.8.0-alpha.10]

Test-channel release. Signed releases show these notes in System before you update.

### Fixed

- Installs and updates now finish on slow connections. A download stops only
  when no data arrives for a while, not because a large file takes long.
- On Ubuntu and Jetson, ElastOS no longer creates files that a later update
  refuses as unsafe. If an existing file is unsafe, the update names the
  command that fixes it.

### Developer detail

- Carrier `fetch_file` and `fetch_content` bound each step (30 s to open, send
  and read an error reply, 60 s for the source's header, then 30 s without
  body bytes) and have no total cap. HTTP release downloads read in chunks with
  a byte ceiling (signed size, else 200 MiB) and a 30 s idle timeout. install.sh
  drops its 30-minute binary cap and keeps curl's minimum-speed rule (1 KiB/s
  over 60 s) (#113).
- Release packaging extracts capsule trees from a file instead of a pipe, so
  macOS bsdtar can no longer stop `git archive` with SIGPIPE (#194).
- The Runtime drops group and world write from its umask before it starts any
  thread (keeping stricter bits such as 077), so Ubuntu's 0002 no longer makes
  group-writable files; an unsafe release file's refusal names `chmod go-w`
  (#279). Installs made by alpha.8 under umask 0002 need `chmod -R go-w` on the
  data folder once, because the installed binary performs the update.

## [0.8.0-alpha.9]

Test-channel release. Signed releases show these notes in System before you update.

### Changed

- Chat is its own signed capsule. A new install downloads it over Carrier, and an
  update from System or Terminal adds it if it is missing.
- Media tools are downloaded again only when they actually change.

### Fixed

- Undo to an earlier release works again. Before, it received the newest
  release's parts and was refused.
- On Ubuntu and Jetson, install.sh no longer refuses the folder it just created.
  If an existing folder is unsafe, it says how to fix it before downloading
  anything.
- Home status no longer shows Content Exchange as "stale install" right after a
  fresh install.

### Developer detail

- Components are fetched by their signed CID, like the Runtime binary and
  components.json; the release name is used only when a descriptor has no CID
  (#287).
- Media tools build with fixed prefixes; the archive is byte-identical across
  build directories (#194).
- `elastos update` in Terminal adds missing Home components only on a Home;
  Terminal and System updates share one support plan.
- Artifact receipts have one format: the bare sha256 hex digest. Receipts that
  earlier releases wrote with the `sha256:` prefix are still read.
- Archive components are verified by their receipt and the recorded hash of
  the extracted executable, and `elastos home status` reports them that way.
  Content Exchange status is correct again (#280).
- One `FileLock` guard unlocks its lock when dropped; `release()` unlocks and
  reports an unlock failure where callers need it.
- install.sh creates its directories 0755 under any umask. Before any download it
  checks the data folder's existing parents with the Runtime's rule (owned by you
  or root, not group- or other-writable). The installer and the media-tools check
  name the unsafe folder and the exact repair command (#279).
- Test fixtures that are executed are written from a child process, so a test
  no longer fails with ETXTBSY.
- Process-group scans skip processes that exit during the scan.
- Runtime and validators accept legacy execution labels and the honest
  `web-projection`, `native-provider` and `native-host` values. Shipped first-party
  manifests retain the legacy values so older Runtimes can complete an update.
  Switch shipped manifests to the honest values in the next release, once every
  supported Runtime accepts them. Runtime audits and Home display actual
  execution boundaries during this transition. Third-party executable admission
  remains gated by publisher, package approval and revocation checks.

### Isolation status

- Apps use opaque browser frames and Runtime capability checks. Home retains
  cross-app capability authority. Providers run as native processes; only the
  model provider is partly confined. Stored keys and full backups remain
  accessible to the host operator. A stolen device or profile key requires a
  new identity. The [isolation plan](https://github.com/Elacity/elastos-runtime/issues/173)
  sets the remaining installed acceptance gates.

## [0.8.0-alpha.8]

Test-channel release. Signed releases show these notes in System before you update.

### Added

- Chat is part of Home. New installs have it, and updating from System adds it.
- This is the first release published for Linux (x86-64) and NVIDIA Jetson. They
  install with the same install.sh command as on Mac.

### Changed

- From alpha.7 on, you update through System, also when a release changes its
  support files. On 0.8.0-alpha.6, System cannot install this update: run
  `elastos update` in Terminal once.
- Updates and installs keep 2 GB of free space on top of what they write. This
  one rule replaces the old percentage checks.
- The installer shows numbered steps. When its output goes to a file or another
  program, it prints plain text.
- Providers no longer carry the release number, so from the next release an
  update downloads only the parts that changed.
- To be able to undo an update, run `elastos source show` first and keep the
  Head CID. `elastos update --rollback-to <that CID>` goes back to it.
- MicroVM capsules are not available in this release.

### Fixed

- Opening Chat again works offline and no longer downloads it again.
- An owner action no longer fails with "Another Home owner action is in
  progress" after Home started another process.
- If an install into another folder is refused, nothing is left behind in that
  folder.

### Developer detail

- Providers no longer embed the release version (`ELASTOS_RELEASE_VERSION`). They
  report their own crate version in startup logs and status, so a new release
  version is no longer a reason for provider bytes and checksums to change;
  dependencies and the toolchain still can change them (#194).
- `crosvm` and the guest kernel are published for no platform, so microVM
  capsules are not available; setup skips them and the supervisor says so. Their
  pinned bytes were unavailable and could not be signed. Release preparation now
  refuses a checksummed component CID without a release file, before signing.
- Disk-space checks no longer demand a fixed share (10% or 15%) of the volume.
  Each step requires that its own bytes fit in the free space. Update staging,
  the installer, model and content downloads and Browser image preparation also
  keep one shared 2 GiB reserve and refuse with "not enough free space: this
  needs N GB plus 2 GB kept free". An update that does not fit keeps the previous
  release. Source-home setup keeps its 16 GiB minimum and the media tools build
  its 4 GiB minimum.

## [0.8.0-alpha.7]

Test-channel release. Signed releases show these notes in System before you update.

**On 0.8.0-alpha.6, System cannot install this update.** Run `elastos update`
in Terminal once. Later updates work from System again.

### Changed

- Home keeps running while an update downloads and checks the whole new version.
  Home restarts only once, at the end.
- If an update cannot be used, your current version stays and System says why.
- ElastOS now installs on Linux (x86-64) and NVIDIA Jetson with the same
  install.sh command as on Mac.

### Fixed

- After an update, protected media could stop opening. It opens again.

### Removed

- Local AI models are no longer part of the release, so Home shows no models for
  now. They will return as a separate signed download.

### Developer detail

- Releases no longer pin or carry a model catalogue; with no pin, Home shows no
  published models. The model CAR retention and import path,
  `model-package-handoff.py`, `fetch-model.sh` and their tests are removed from
  release preparation, signing and publication.
- Setup reads a pinned model catalogue over Carrier with its 128 KiB bound and
  refuses a larger declared size before reading the body.
- Setup accepts passive content capsules from signed releases and checks provider
  roles and icons. It refuses URL-only downloads and blank release paths,
  including entries with a CID. Explicit CID-only operator gateways verify
  checksums before replacing installed bytes.
- Fresh setup admits the private signed release pair and binary under the
  installation writer, then fetches components and the model catalogue over
  Carrier and verifies both before writing metadata or components. Signed
  manifest bytes stay unchanged.
- CLI updates check the candidate executable before installation with a bounded
  version probe. Failed launches, timeouts, excess output and unexpected version
  output preserve the previous installation.
- An offline update foundation stages and verifies release files with unchanged
  support assets, an installation lock and journal, and restoration after failed
  or interrupted activation.

## [0.7.1] - 2026-09-30

Monthly developer release of the 0.7.1 work line. It is a source snapshot for
developers and testers, not a release for users. The workspace version moves
to `0.7.1`; capsule manifest versions do not change. The GitHub binaries are
unsigned source builds and report `0.7.1-dev`. The signed installer and the
update channel do not change. An earlier signed preview build also reports
`0.7.1`; it is older code, and the next signed release replaces it. The first
release for users is planned as 0.8.0, and issues #83 to #93 track what it
still needs.

### Added
- Local models: signed model catalogue metadata, verified model content that
  is prepared, admitted, kept and retired through Marketplace and System
  controls, and a bounded local llama engine with recovery. Local chat is
  bound to the engine's context limit and keeps the negotiated chat roles.
- Remote and hosted models: granted remote model offers and runs travel over
  Carrier and are authorised on the owning Runtime. Hosted runs support
  Responses with honest cancellation, keep repeatable connections and their
  secrets on this Home, and show the offer and selection facts in Assistant.
- Home: first run seeds an empty desktop and pins Marketplace; new lock face;
  storefront and one Home entry; the recovery kit comes before passkey setup;
  apps declare their own window policy; the Home Agent runs as its own capsule
  on the typed model contract.
- Browser: remote Engine sessions go through Runtime authority; the local
  Engine image dependencies are verified; scoped operator inspection and
  reference fill need approval.
- Services shows the AI model service beside Engine and Exit.
- Protected content: installed custody and chain prerequisites can be
  provisioned. The protected-content path stays inactive.

### Fixed
- Update tests observe isolated CLI and operator update hops accurately
  (#74, #79).
- The x264 build fetch is pinned to its commit and tree (#119).
- Browser, model, setup, Services and Assistant fixes; see the commit log
  since v0.7.0.

### Known limits
- A hosted model route set up explicitly by an operator on a Mac can skip
  Inbox (#85).
- Signed updates have open defects (#107). A later update may need a
  reinstall.
- Chat sign-up is hard to find (#86).
- The Jetson Runtime install and its visible Assistant reply are not proven
  yet (#90, #97).

## [0.7.0] - 2026-08-31

The coordinated workspace version moves to `0.7.0`. Capsule manifests changed
since 0.6.0 take a minor bump (`0.1.0` to `0.2.0`; `wallet-provider` to
`0.3.0`); capsules new in this release keep their initial manifest versions.
Installed artifacts report `0.7.0` only when built through the checked publish
flow with `ELASTOS_RELEASE_VERSION`; unstamped source builds report
`0.7.0-dev`.

### Added
- The 0.7 source candidate integrates the reviewed protected-content
  lifecycle with collaboration, Home/platform, Wallet, GBA, model-provider,
  Assistant, Library, Marketplace, and player UIUX. The Runtime-owned
  protected-content path remains inactive until installed proof and cutover.
- Protected-content creators publish one canonical Chain-bound listing package
  through content addressing. A buyer Runtime imports the shared listing,
  verifies it, and uses the same immutable projection for purchase and playback.
- The source proof now covers two distinct Runtimes and real process-backed
  2-of-3 custody nodes. The creator Runtime provisions the nodes; the buyer
  Runtime completes import, deny-before-buy, purchase, open, read, and close
  through its own Profile, Wallet, device identity, and signed release.
- A bounded read-only protected-content audit verifies source and installed
  artifact parity, private provider declarations, profile and role facts, and
  operator prerequisites. Its redacted receipt reports readiness for active
  proof separately from product readiness.
- Full source-home setup installs one stable Runtime under the platform data
  root and writes an owner-only receipt that binds source identity, Runtime,
  components, capsule metadata, platform, and installation time.
- macOS and Linux source-home restart helpers validate the stable installation
  receipt and exact prior process before replacement. They own one PID file,
  one bounded principal-root rollback, and one atomic restart receipt.
- People now reports Profile readiness explicitly. Passkey registration and
  Profile setup are separate: a valid passkey remains valid when Profile setup
  is not ready, and People directs the person to System Recovery rather than
  hiding protection or Recovery changes behind a profile save.
- The privacy-reviewed Home journey audit is available under `docs/audits`.
  It records findings and test coverage; prerequisite-blocked and partial
  observations remain separate from completed tests.

### Added (release tooling)
- `scripts/publish-release.sh --dry-run` rehearses a publish entirely
  locally: no network reads or writes, no IPNS publish, no public URL,
  and the `last-release-*` publish state stays untouched. `--key` becomes
  optional; an ephemeral throwaway key signs the rehearsal artifacts.

### Fixed
- The release publisher now stamps `model-provider`, `assistant`,
  `elacity-player`, and `gba-nonogram` direct assets; their
  `components.json` release entries previously stayed without checksums.
  The drifted `wallet-walletconnect` entrypoint and stylesheet hashes in
  `components.json` are restamped from the current source bytes.
- Runtime's executable watcher skips whole-binary hashing while the file's
  metadata is unchanged. Startup and changed-file checks still verify the hash.
- A mint retry adopts a fully completed durable record after a crash loses its
  intent completion mark. It verifies the complete custody selection and
  availability evidence. Partial or ambiguous records require reconciliation.
- Home audit repairs cover Terminal startup and Inbox handoff, Recovery status,
  window placement, create-only Library writes, document close protection,
  Assistant output/clipboard handling and model-provider initialization.
- Source-home setup removes its private install stage after success or a
  controlled copy/installer failure. Setup checks source and data volumes for
  at least 10% free capacity. Setup and Browser refresh retain one default VM
  backup set; a refused restart preserves existing recovery data.
- Runtime diagnostics omit private VM initialization payloads and provider
  responses. Log-capture tests cover the privacy boundary.
- CI and local test recipes cover the own-workspace capsule suites and required
  provider processes. Fixtures isolate Carrier routing from public relay
  availability, retry transient executable-busy errors and reap their children.
- Bound `browser-local-exit` to its launching Runtime through a held-open stdin
  pipe. Cleanup now checks the relay socket inode, and Runtime refuses to
  replace a socket still owned by a live helper. Runtime abort, exit, rebuild,
  and supersession paths no longer leave the helper orphaned.
- Renaming yourself no longer looks like becoming someone else. A person's
  identity is their Profile DID and a rename only advances the revision, but
  the context check compared whole signed Profile documents, so editing your
  own name read as a different authority and your Home began refusing
  incoming messages from contacts who had done nothing. The check now asks
  the question that matters — same Profile DID, and no rolled-back revision
  replaying an older truth — and a newer revision of the same DID is
  accepted as what it is: the same person, saying something new about
  themselves.
- A running Home announces the people who live on it. Presence was
  published by a browser tab on a timer, once per tab, and disappeared when
  the tab closed, so two Homes that were both running showed each other
  Offline. Everything the announcement carries is durable — the principal
  record and its signed Profile — and the Runtime now publishes it for
  every Profile on the Home. The announcement still has a lifetime and is
  still refreshed on the protocol's cadence, but once per Home rather than
  per open tab, and whether or not anyone is looking.
- A running Home receives its owner's messages. Delivery contexts existed
  only while a signed-in browser session held one, so a Home with nobody
  looking at it refused messages from contacts it had already accepted:
  the sender saw "Sending" for minutes, and the queue drained only when the
  recipient opened Chat, which made a retry loop look like the transport.
  The Runtime now registers each Profile on this Home at startup, so
  acceptance rests on the durable relationship and a verified envelope
  rather than on a browser being open. Sending still requires the person's
  session, because sending acts on their behalf. Measured on two Homes with
  no tab open on the receiver: the recipient Runtime accepted it and the send
  settled in six seconds.
- Carrier and direct-message failures keep their cause. Both sides of the
  peer call discarded the underlying error, so a message that would not
  send reported only that it had not sent; the reason now reaches the log.
- Chat windows use the standard window bar. The unified-sidebar chrome hides
  the bar because the app's own left sidebar stands in for it, and Chat was
  opted in while its sidebar was still unbuilt, leaving the window controls
  floating over bare content. That chrome is now opt-in by its class alone,
  and the lone "Shared room" tab no longer renders as a full-width button:
  a selector appears only once a direct conversation exists to switch to.
- Profile creation now fails clearly when the principal root is not protected.
  People opens System Recovery for that prerequisite; Profile setup itself does
  not establish protection, mint Recovery material, or weaken a valid passkey.
- Home state directories are created owner-only. The shell writes into a
  person's root at its first mount, `create_dir_all` follows the process
  umask, and protected writes then refuse the world-readable parent the
  Runtime itself had just made — a Home that had drawn its desktop could
  never save anything protected. Creation now narrows to owner-only inside
  the data directory, and never reaches above it. The check that refuses a
  loose parent at write time is unchanged.
- App windows are visible again. `.window-frame` starts transparent so a
  capsule never flashes white before it paints, and its rule promised a
  fade-in on load with a fail-open behind it, but nothing ever restored the
  opacity: every window since the shell redesign painted correct, fully
  styled, interactive content into an invisible frame, so the desktop, the
  chrome and the title drew while the app never appeared. Load now reveals
  the frame and a 1.5s fail-open reveals it regardless. The launch skeleton
  had the same shape of bug — its rule says it is hidden the moment the
  iframe reveals, and nothing hid it — so a grey square pulsed over every
  app forever; it now hides on that same signal. The passkey smoke's app
  matrix counted DOM nodes, which is markup and not paint, and passed on
  eight invisible windows; it now requires each window to become visible
  within the reveal's own budget, proven by deleting the rule and watching
  it fail.
- A Home session the Runtime will not renew sends a person to the passkey
  gate instead of a signed-looking desktop of empty windows. Boot swallowed
  the refresh failure and trusted a summary that can still read as signed,
  which left no way back in.
- Session refresh rotates the Home cookie while open tabs keep the prior
  mint, so a divergent header/cookie pair is no longer a conflict when both
  tokens verify to the same session authority. Genuine conflicts still fail,
  now as 403 so a client bounces to the gate rather than erroring.
- A first Recovery Kit can be created on a Home that has already rendered
  its desktop. Activation demanded a clean principal root while the offline
  migration path demanded protection that could not exist yet, so any Home
  that had drawn the shell was locked out of protection — and with it
  Profile authority and all of People. Activation now migrates the surviving
  plaintext under its own guard, backing the originals up first.
- Direct messages send under the authority a person's session actually
  carries. The check required the grant to name `chat-room`, but passkey
  sign-in only ever mints grants naming Home and System, so no real person
  could send; the API tests passed only because their fixture fabricated
  grants the product never issues.
- A first run no longer republishes presence it knows will be refused. The
  Runtime answers 409 until a Profile exists, and the heartbeat now stops on
  that answer instead of filling the console every interval.

### Changed
- Product inventory exposes one Chat app. The Runtime-backed `chat-room` app is
  the only Chat entry in manifests and supported release profiles. The
  obsolete terminal Chat and Agent capsule source trees are retired. Explicit
  operator CLI commands remain under their own contract; Git history preserves
  the old implementations.
- Runtime keeps `tracing`; the replacement `elastos-logger` is postponed.
  Advanced Assistant workflows, broader Carrier reconciliation and the legacy
  audit migration remain deferred. Installed protected-content acceptance and
  the atomic activation/removal of provisional providers remain open.
- Collaboration messages now identify people as Profiles and recipients as
  Profiles or conversations. Runtime derives routing from the currently signed
  Profile instead of putting endpoint identity in the product message. Endpoint
  acceptance receipts remain endpoint receipts, so Chat shows `Sent` rather
  than claiming the person received or read the message. For direct messages,
  contact revocations, and Profile updates, the receiving Runtime now derives
  the remote endpoint from the authenticated Carrier connection. It verifies
  that route independently from the Profile-authorized application signer, and
  rejects caller-supplied or substituted endpoint metadata before persistence.
- Shared-room Chat and presence now carry one strict Profile-authenticated
  payload: `{ product, signed_profile }`. Runtime request binding covers that
  exact envelope, admission verifies the Profile's endpoint and scoped signer
  grants, and product code receives the verified Profile plus only its own
  inner payload. Names and presence therefore come from signed Profile truth,
  not device labels or fields repeated inside Chat and presence payloads.
- The Services peer contact object is named as what it holds. The released
  `people-contacts.json` (schema `elastos.people.contacts-state/v1`) carried
  Services peer data under a People name; it is now
  `services-peer-contacts.json` with schema
  `elastos.services.peer-contacts-state/v1`, and an installed host's object
  migrates whole — name and schema together, through the protected-object
  path since the encryption binds the object URI — on first touch. The
  capsule-facing read-model audit the task list called for came back clean:
  device identity serializes to the System diagnostics surface only, remote
  endpoint DIDs are serialization-skipped, offer routes are app navigation,
  and the one topology field (`source_peer`) is the System trusted-source
  panel's by design. Offline reach limits are now stated in one place for
  all three transports: direct messages end terminal and visible, the shared
  room's gossip buffer bounds catch-up, and Profile updates bridge at most
  the 8-revision announcement ring before failing closed to a fresh
  approval.
- Every request/response collaboration retry now runs on one delivery
  engine. `collaboration_delivery.rs` owns the bounded pass — attempt the
  whole plan, settle only what the selected peer endpoint acknowledged, report the
  first failure without letting one unreachable peer starve the rest — and
  direct messages, contact revocations, and Profile update announcements all
  run on it instead of three hand-rolled loops. What happens at an envelope's
  end of life is now a declared per-source decision pinned beside the code
  that implements it: terminal and visibly expired for messages, re-mint the
  exact signed fact for revocations, regenerate from durable truth for
  Profile announcements. Artifacts stay in the store their authority owns,
  and the shared-room outbox remains the one stated difference (broadcast
  with asynchronous receipts). Revocation delivery failures now propagate to
  the sync scheduler instead of being silently swallowed, so failed passes
  back off like every other delivery. The encrypted mailbox lands on this
  layer instead of adding a fifth loop.

### Added
- Recovery now restores the collaboration identity. The Full Recovery Bundle
  carries the signed People identity — the Profile signing seed with its
  retained revision ring, and the signed contact store — alongside the root
  kit and Wallet keys. Importing it on a genuinely fresh machine keeps the
  same Profile DID, restores the contacts, and authorizes the new device by
  minting the next signed revision through the normal Profile authority path,
  which the existing update-delivery chain then announces to every accepted
  contact: relationships survive the machine. The import response and the
  System recovery UI report the People outcome honestly and never claim a
  complete account restore when the identity did not come back.

### Changed
- Direct conversations are text-only by declared decision. Attachments need
  object handling, retention, and a delivery path of their own — designed on
  the unified delivery layer together with the encrypted mailbox, not as
  another ad-hoc transport — so until then the attach control in a direct
  conversation is visibly disabled and says why, instead of silently missing,
  and the product doc scopes attachments to shared conversations.

### Added
- Home now tells you when someone wants to connect or has messaged you. A
  pending contact request appears in the durable notification store — one
  projection read by the Home badge, the toast, and Inbox alike — and is
  pruned the moment the signed request is decided, revoked, or expired. A
  verified incoming direct message mints one live notification per
  conversation, named by the sender's signed Profile presentation, resolved
  by reading the conversation in Chat and resurfaced by new mail. Inbox stays
  the only Accept/Decline surface: notifications point at it and never carry
  the decision.
- Signed Profile update delivery now works between runtimes. A rename (or an
  authorized-device change) travels to accepted contacts as an exact bounded
  signed revision chain over the dedicated `collaboration-profile` Runtime
  provider, and the receiving store applies it only under the strict rules:
  an already accepted Profile DID, the next exact revision bridged by the
  exact previous signed-envelope hash, a delivery envelope signed by an
  application signer the new head authorizes, and delivery from the endpoint
  the new head names — rollback, gaps, conflicts, mixed profiles, and
  unauthorized signers or endpoints fail closed. Announcement is a pure function of the
  local head and the accepted contacts, so a restart simply re-announces and
  the receiver treats it as an idempotent replay. The two-runtime proof
  surfaced a real wire gap: the Carrier peer provider plane's allow-list had
  never admitted the profile provider, so no announcement had ever been
  deliverable; it now is.
- Shared-room names now come from signed Profile truth. Every configured-room
  participant and message row with a verified member device is named from the
  polling principal's own signed Profile, an accepted (or retained removed)
  contact head, or a room membership profile card — and a verified device none
  of those name renders as an explicit "Unverified device" row instead of a
  stored `Device {hex}` label, a presence self-claim, or an invented
  "Conversation member N" placeholder. Presence heartbeats stay liveness-only,
  durable conversation history is untouched, and the plain (unconfigured) room
  keeps its server-stamped home-session and guest names.
- Bilateral signed contact removal, with the complete People relationship
  states. Removing a contact mints a pair-scoped signed revocation: removal is
  immediate locally, the Runtime durably retries the exact envelope until the
  peer's device acknowledges it (re-minting only after an envelope's own
  lifetime), and the peer's side verifies it against the pair's accepted
  authority. Both sides keep the relationship visible in a removed state —
  named by the retained signed Profile head — instead of letting it vanish.
  Removed permits reading only: history stays readable under an explicitly
  declared policy, sending and receiving stop on both sides, and a fresh
  request through Inbox reopens the pair. People now also projects `requested`
  and `declined` from the signed request and decision chains, and the store
  binds retained Profile heads to known relationships only.
- People shows whether an accepted contact is reachable right now. The signal
  is the existing presence heartbeat projected onto the contact list — Online
  now / Offline when presence has an answer, nothing when it has no basis —
  and it stays out of the realtime signature so heartbeat flaps do not emit
  people.changed events.
- The system bar carries the focused app's name and that app's own menus. Apps
  declare them with a postMessage manifest and receive each chosen command
  back; an app that declares none gets File with Close Window. Manifests are
  data, not authority: the host takes one only from an app frame, binds it to
  that frame's launch token, and never accepts a window id from the sender.
- Spotlight, opened with Cmd/Ctrl+Space, searches open windows, installed apps
  and desktop objects together. It reads only what the shell already holds, and
  opens objects through the same capability guard the desktop uses.
- The shell keyboard layer: Alt+Tab window switching, Cmd+` cycling, Cmd+W /
  Cmd+M close and minimize, Cmd+Alt+arrow snapping, F3 for Mission Control,
  Ctrl+arrows to switch Spaces, and Cmd+/ for a shortcuts overlay that
  documents exactly what exists. Quick Look arrives with it: Space on a
  selected desktop icon opens a frameless preview of glyph and metadata —
  never object bytes, which stay behind viewer authority — and Open routes
  through the same guarded path the desktop uses. The layer sleeps whenever
  Home GUI is not the mounted shell.
- Stages, Desktop Spaces and Mission Control. The green button gives an app its
  own fullscreen Space, extra Desktops can be added, and Mission Control lays
  the Spaces out as live thumbs over the open windows — click to preview, click
  again to enter, drag to reorder. Sessions restore the Space you were on.
  Presentation only: a Space switch mints no authority.
- Capsules declare their own app icon. A capsule names an icon directory in its
  manifest, the Runtime resolves it to that capsule's own asset routes, and the
  shell renders what it is handed instead of keeping a central icon table keyed
  by capsule name. Capsules that declare none get the shell's generic glyph.
  People, Chat Room and Inbox ship their own icons.
- One canonical source for the shared UI tokens, theme runtime and accent
  picker, stamped into each participating capsule by `just vendor-ui` and gated
  against drift in `just verify`. Per-capsule origins make the copies necessary;
  the gate makes them identical.

### Changed
- The local device DID reaches exactly one browser surface: System, the
  runtime-inspection page. The Home and People read models and the People
  profile-update response no longer serialize it — no other surface consumed
  it, and device identity stays out of app-facing projections.
- The People contact read model drops its dead `route` field. Nothing read it,
  the Profile path always left it empty, and route strings do not belong in
  capsule read models.
- The Home GUI shell takes the redesigned look for the surfaces it already has:
  desktop, dock, windows, launcher, toolbar and notification toast. It loads the
  shared token sheet, so it follows theme and accent like the app capsules.
- People, Chat Room and Inbox take every colour from the shared token sheet
  rather than their own palettes, so all three follow theme and accent. Only the
  colour source moved; their layouts are unchanged.
- The shared theme runtime owns no browser-profile storage. It keeps an
  in-memory view and accepts a persistence adapter that only a host installs,
  which lets the opaque Home GUI take the shared tokens without violating the
  rule that capsule frames do not own that storage.
- People's sidebar behaves as the tabs it looks like: sections switch and the
  page title names the visible one, replacing a scroll between two
  always-visible sections.

### Fixed
- A contact that never acknowledges can no longer wedge the direct message
  store. Abandoned records — expired with no receipt — are terminal for
  retention: settled pairs prune first, abandoned records yield next, and only
  a store made entirely of live unexpired messages refuses a write as honest
  backpressure.
- A direct message abandoned at its 24-hour TTL now reads `expired` instead of
  `pending`, and the Chat UI shows "Not delivered" instead of "Sending". The
  Runtime already stopped retrying at expiry; the read model and UI now say so.
  A settled receipt still wins: an acknowledged message stays Sent even
  when read after the TTL.
- Home summary reads are side-effect free. Reading the Home summary no longer
  materializes or rewrites the shared-room store or the notification store; a
  read now answers from an in-memory expiry view and persists only a real
  change.
- Chat Room text on an accent fill uses the accent's own ink colour instead of
  hardcoded near-white, which was unreadable on the yellow and graphite
  accents.
- People no longer presents a contact state it does not recognise as
  "connected". The Runtime emits two relationships today; anything else now
  renders as an unknown state, so a build that has not learned a new state
  cannot tell someone they are still connected to a person they may not be.
- The Home GUI dock draws its Apps launcher glyph again. It had been pointed at
  an icon file the shell does not ship.

## [0.6.0] - 2026-07-31

0.6.0 combines the reviewed ESP line with the Wallet, Recovery, and Browser
continuation. These changes are not part of the 0.5.0 release history.

### Changed
- Added the ESP v0 descriptor, Runtime-derived capsule/interface catalog, and
  fail-closed shell selection for the allowlisted Home GUI and Home CLI shells.
- Split `/apps/home/` into a neutral host, trusted graphical shell modules, and
  an isolated Runtime-owned Home CLI PTY while keeping both shells on the same
  catalog, intent, approval, and provider facts.
- Replaced first-party WASI product entrypoints with explicit Runtime
  projections and added a test-only `elastos.component/v1` conformance fixture
  for the `elastos:bus@v1` authority path.
- Added locked, isolated, path-remapped Component artifact builds and made
  setup and release packaging use that same build path.
- Restored the GBA viewer/content relationship as a portable browser projection
  and tightened Browser audio/session diagnostics without adding a fallback
  rendering path.
- Added canonical capsule authoring templates, manifest/interface checks, and
  active-product inventory gates.
- Extracted People from Home into a standalone first-party app capsule with an
  app-scoped launch token while keeping profile, discovery, requests, contacts,
  removal, and Chat handoff Runtime-mediated.
- Added signed Home launch-token v4 authority, the typed Wallet Bus v2.3
  boundary, durable exact-intent passkey step-up, managed Wallet recovery, and
  Runtime-owned transaction effects that reconcile without rebroadcasting.
- Added the macOS VZ Browser product path with Runtime-only networking,
  WebRTC display, explicit Browser Engine and Exit selection, principal-scoped
  profile ownership, and durable page/VM cleanup ownership.
- Added a Runtime-mediated injected-wallet bridge. Deterministic localhost proof
  confirmed `window.ethereum`, EIP-6963 discovery, the Runtime Wallet binding,
  and one `eth_requestAccounts` call creating one pending account-access request.

### Fixed
- Bound `browser-local-exit` to the lifetime of the Runtime that launched it via
  a held-open stdin pipe, so it no longer survives as an orphan when the Runtime
  is SIGKILLed, aborts on panic, or leaves through `std::process::exit` (the
  installed-binary supersession watch takes that path on every rebuild). Helper
  teardown is now scoped by inode identity to the relay socket it bound, and the
  Runtime refuses to replace a relay socket a live helper is still serving
  instead of stranding it on an unlinked socket.
- Fixed Home launch classification so browser projections are attached as
  authorized web surfaces instead of being sent to a WASM compute provider.
- Bound fresh passkey authority to one app, operation, and request payload;
  made Inspector approval claims atomic; and made persisted audit records
  Runtime-signed and hash-chained.
- Isolated shell and app frames with opaque browser sandboxes, removed
  ambient-cookie token minting, trusted Host-header callbacks, and manifest-only
  shell admission.
- Made passkey session refresh atomically revoke its predecessor, restricted
  first-owner enrollment to local Runtime access, and removed unauthenticated
  capsule bootstrap plus obsolete standalone Recovery Kit routes.
- Rejected archive links and host-path escapes during capsule install and
  browser asset serving.
- Bound Browser Wallet reads and approvals, exact EVM signing outcomes, managed
  approval authority, and recovery state to the accepted Wallet contracts.
- Made Browser close and authority renewal acknowledged and retryable, retained
  exact Runtime cleanup ownership across interruption, gated the deterministic
  close handshake in `just verify`, and removed a delayed terminal Home window
  after the exact close result.

### Known limitations
- Browser is included in 0.6.0, but is not claimed fully reliable. The accepted
  localhost proof covers launch, display, navigation, and injected account
  access; it is not a general product-readiness claim for every target or site.
- Browser restart remains intermittent: one observed restart failed and the
  next opened. The `ela.city` login did not survive the restart, and current
  Browser performance is slow.
- Browser profile storage remains principal-owned and reset-scoped, but is not
  yet protected or Recovery Kit-recoverable.

### Deferred
- Carrier reconciliation and physical multi-node evidence are deferred to 0.7.
- The shell UI redesign and extended AI UI work are deferred unless reviewed as
  independent post-release changes; they are not part of this 0.6.0 code tree.

## [0.5.0] - Unreleased

0.5.0 was the baseline before the 0.6.0 release. It brought the Mac, Jetson,
and server work into one line while keeping Browser readiness claims tied to
target-device media, audio, input, and installed-path proof.

Notes from the unpublished intermediate patch line are folded into this entry
because no separate patch release was published.

### Added
- Added the first Services app so local services start disabled, can be enabled
  intentionally, and remote services from trusted people are shown separately.
- Added opt-in People discovery, `elastos://peer/invite?...` pairing, trusted
  people, display names, and the 1:1 chat path while moving service management
  out of People.
- Added service registry flows for enabling local services, requesting access to
  services from trusted people, and approving remote Browser Exit use.
- Added Browser Engine and Exit selection in Browser so non-KVM hosts can serve
  the Browser UI and use approved remote engines instead of pretending to be
  local VM providers.
- Added Browser Session Manager capacity receipts, page ownership checks,
  heartbeats, stale-session cleanup, and fail-closed page control.
- Added Mac VZ and Linux crosvm Browser VM target support, including WebRTC-only
  display, VM artifact preflight, target refresh, runtime relay setup, active
  target audits, and remote VM launcher support.
- Added Runtime-owned Browser relay helpers for WebRTC and private stream
  wiring, including TURN setup when `turnserver` is available.
- Added Browser profile-disk descriptors and principal-owned reset handling,
  with the storage truth documented as reset-scoped but not yet encrypted or
  recoverable.
- Added Inbox fresh-passkey approval for built-in Wallet requests while keeping
  Wallet as the review surface for wallet authority.
- Added the Inspector approved-provider dispatch surface with fresh passkey
  approval and typed provider invocation boundaries.
- Added public install and release proof helpers for component checksums,
  publisher bootstrap integrity, source-home setup, source-home restart, and
  installed-path smoke testing.
- Added the public `AGENTS.md` operator contract and onboarding docs for review,
  setup, target Browser proof, People conversations, and Inspector testing.

### Changed
- Reworked Home authority boundaries around passkey principals, launch tokens,
  localhost `Users/self` mapping, protected content providers, Browser page
  ownership, and sanitized provider responses.
- Removed Browser `runtime_frame`, `diagnostic_frame`, screenshot, and
  image-polling fallbacks from the product path. Browser launches now use
  WebRTC remote display only.
- Clarified the server role on non-KVM hosts: serve Home and Browser UI, then
  delegate Browser execution to approved remote engines when needed.
- Hardened Browser VM preflight and setup so stale Unix sockets no longer count
  as launch-ready and gateway-only hosts keep remote VM wiring instead of being
  overwritten with unusable local crosvm defaults.
- Hardened Browser Wallet and dapp bridge behavior so Wallet reads, prepare,
  signing, and broadcast stay Runtime-mediated and reviewed through
  Wallet/Inbox.
- Cleaned the System, People, and Services app surfaces so first-party app
  navigation is quieter, People focuses on people, and Services focuses on
  manageable services.
- Aligned the default publish/setup profile with the current Home surface,
  including System, People, Services, Browser, Wallet, Documents, Library,
  Marketplace, Archive Manager, and Inbox.
- Tightened release publishing so `elastos publish-release` uses the explicit
  `home` profile by default, demo-only capsules stay behind the `demo` profile,
  and first-party artifacts require sha256 or sha512 checksums before signing or
  serving.
- Hardened public-install smokes so they pin the installer-selected components
  manifest and fail unless the selected gateway serves a 0.5.0-compatible
  `home` profile with checksummed artifacts.
- Hardened trusted-source Carrier bootstrap stamping so installer refresh and
  release publishing require one publisher-scoped ticket/node pair.
- Removed private staging, reconciliation, and local-machine proof wrappers from
  the public script surface. Public proof now names reusable gates and explicit
  target checks instead.
- Updated release, install, Mac, Browser VM, People, Services, Inspector, and
  architecture docs so proof claims distinguish source gates, installed-path
  checks, target-device proof, and Browser product acceptance.

### Fixed
- Hardened Home app launch materialization so release-installed app bundles can
  be refreshed from signed package metadata, stale bundles missing their
  declared entrypoint are detected before launch, and source/dev launches remain
  local when no release package identity is present.
- Added macOS ARM64 setup metadata and Home runtime transport fixes so managed
  Home can reuse a live local runtime, route WASM carrier calls through the host
  runtime when FIFO bridge transport is unavailable, and avoid Linux `/proc`
  process-liveness assumptions.
- Cleaned the capsule catalog count construction so strict workspace clippy
  gates pass with `-D warnings`.
- Fixed Browser VM supervisor autostart smoke cleanup so fake launchers are
  reaped after each run instead of leaving orphaned `node -` processes.
- Fixed public install identity smoke permissions so the documented installed
  DID/profile proof reaches the publisher check instead of failing locally with
  `Permission denied`.
- Fixed public installs to refresh trusted-source metadata from the live
  Publisher Carrier bootstrap route instead of relying on stale publish-time
  peer tickets.

### Verification Status
- Source/review proof should cite concrete commands: `git diff --check`,
  `node scripts/home-entropy-check.mjs`,
  `node scripts/browser-entropy-check.mjs`,
  `bash scripts/check-wci-alignment.sh`, `just candidate-command-audit`, and
  touched Rust or capsule tests.
- Auth, wallet, chain-authority, Inspector, Services, People, and Browser wallet
  bridge smokes are part of the required review proof set.
- Target Browser VM parity proof remains operator-supplied. Use
  `scripts/jetson-browser-runtime-audit.mjs` with explicit target host, user,
  data directory, and source checkout arguments.
- Public install proof still requires a staged or published 0.5.0-compatible
  manifest with the current `home` profile and checksummed artifacts, followed
  by `scripts/public-install-identity-smoke.sh` and
  `scripts/public-install-home-frontdoor-smoke.sh`.
- Product Browser completion is not claimed until
  `scripts/browser-objective-audit.mjs` has accepted product media proof plus
  hash-bound manual UX evidence.
- Manual installed-device checks on Mac and Jetson are still required before a
  release handoff: `elastos setup`, open Home, visit first-party apps, and
  return Home cleanly on the installed path.

## [0.4.0] - 2026-06-09

### Added
- Added a provider-backed Library release slice: desktop-familiar
  places, breadcrumbs, grid/list views, inline create/rename, drag/drop,
  preview/open, upload/download, folder and selected-object archive download,
  provider-created ZIP objects, safe `.tar`/`.tar.gz`/`.tgz`/`.zip`
  extraction, publish/share/status/repair, Trash, browser Back/Forward
  takeover, and desktop-style context menus now run through Runtime-scoped Library
  provider authority.
- Added `object-provider` as the mutable principal-root object provider for
  Library files/folders, Desktop/Documents/Public roots, protected-root object
  envelopes, object events, legacy plaintext auto-protection, and WebSpace
  resolver routing. Runtime registers the canonical `object` provider scheme,
  and browser calls use `/api/provider/object/*`.
- Added Library-to-Documents viewer handoff and Home Desktop projection so
  concrete Library objects can open in Documents and Desktop file mutations stay
  provider-owned while Home only displays and launches them.
- Added the current Spaces/WebSpace contract for Library:
  `localhost://WebSpaces/*` is the local mounted resolver view shown as
  **Spaces**, provider targets such as `cloud://drive/...` remain
  resolver-private, read-only mounts hide mutable actions, mutable mounts/forks
  can materialize provider-owned objects, and `elastos://content/*` remains the
  provider-independent published/shared content surface.
- Added the first Carrier-backed content availability proof path for Library
  publish/status/repair: `content-provider` signs availability receipts with
  peer-selection, quota, repair-worker, accounting, and abuse-control metadata;
  built-in Carrier availability can invoke remote service providers for
  `content/ensure`, `content/status`, manifest-backed object import, and
  exact-CID byte fallback without exposing raw Carrier tickets, Kubo/IPFS, or
  peer handles to apps.
- Added Runtime provider-to-provider invocation foundations, including typed
  invocation envelopes, transfer receipts, range/progress propagation, a
  validated bounded `ProviderTransfer::Stream` base64-chunk envelope, and
  service-provider-only Carrier `provider_invoke` transport.
- Added Runtime-native provider stream sessions on top of
  `ProviderTransfer::Stream`: providers can be opened as read/cancel sessions
  with live progress events and read-next backpressure, `content-provider`
  fetch consumes local IPFS and availability fallback reads through that
  session path, and Library object downloads now return chunked HTTP body
  streams with backpressure/cancel transfer receipts.
- Added the WebSpace federation slice: `operator-drive-adapter` now has an
  operator-private endpoint backend contract with redacted config/status,
  provider-to-provider invocation boundaries, durable cache/viewer handoff
  coverage, mutable fork write-back, and a filesystem-backed endpoint proof for
  metadata traversal, byte reads, and mutable writes.
- Added recipient-scoped Library share-grant records, `shared_access` checks,
  access-decision/shared-open receipts, and fail-closed key-release policy
  handling as the current local/provider-mediated sharing slice.
- Added the protected recipient receipt-chain proof: Library can publish a
  non-production `protected_content_fixture` sealed-object descriptor through
  `content-provider`, record recipient-scoped key-release grants, bind
  `shared_access` to Runtime recipient proof plus launch `session_id`, invoke
  DRM/rights/key/decrypt providers, return a viewer-scoped protected-open
  contract, and fail closed when protected-content providers are absent without
  exposing raw CEKs, plaintext, wallet, chain, Kubo, host filesystem, or
  provider credentials to apps.
- Added durable storage accounting: `content-provider` projects
  signed availability receipts into a persistent per-principal ledger, exposes
  storage-accounting summaries and no-settlement storage-market posture through
  `content/status`, and preserves original publisher identity when unpublish is
  called without explicit principal metadata.
- Added principal storage-quota admission for content publish/import:
  `availability_requirements.max_storage_bytes_per_principal` is enforced from
  the durable content accounting ledger before local content-backend writes and
  recorded as `principal_storage_quota` posture in receipts/status.
- Added bounded cross-provider content admission preflight: `content-provider`
  exposes provider-only signed `elastos.content.admission/v1` receipts, and
  Carrier invokes and verifies remote `content/admission` before
  `content/ensure`, exact/object import, or block-graph repair transfer so
  unsigned admission, payload mismatches, and quota rejections fail closed before
  bytes move.
- Added an optional storage-market endpoint-quorum admission gate for `content/admission`:
  `ELASTOS_CONTENT_STORAGE_MARKET_ADMISSION_*` can point content-provider at one
  operator-owned admission endpoint or a bounded configured endpoint set with an
  explicit quorum, and accepted/rejected market decisions are normalized into
  the signed admission receipt before Carrier moves bytes or DAG repair data.
  Credentials are redacted from status, and endpoint or quorum failure rejects
  admission fail-closed.
- Added optional external repair-fleet dispatch for the Runtime-gated
  `content repair-worker`: `ELASTOS_CONTENT_EXTERNAL_REPAIR_FLEET_*` can point
  content-provider at one operator-owned dispatch endpoint or a bounded endpoint
  set with an explicit quorum, due tasks are sent as
  `elastos.content.external-repair-fleet.dispatch-request/v1`, replies are
  normalized into dispatch receipts with endpoint receipts plus quorum counters,
  and credentials are redacted while local provider verification still decides
  final availability.
- Added bounded Carrier repair-graph policy receipts: current cross-peer repair
  explicitly supports object-manifest and exact-byte import fallbacks while
  refusing arbitrary IPLD DAG fallback unless the Runtime-only block-graph
  provider ABI is available.
- Added the block-graph provider path: Runtime reserves
  `elastos://block-graph/*`, the build/release surfaces include a
  `content-block-graph-provider` contract capsule, startup registers the
  verified provider when installed, and Carrier routes arbitrary `ipld_dag`
  repair through local `export_graph` plus remote `import_graph` provider-plane
  invocations. The provider uses the `ipfs-provider` Kubo coord file to export
  and import bounded DAG CAR bytes, pins imported roots, and fails closed when
  Kubo/provider setup is absent.
- Added `elastos content status` for operator inspection of provider-wide or
  per-CID availability, storage-accounting, quota, repair, peer-proof, and
  no-settlement storage-market status through Runtime provider invocation.
- Added `elastos.content.repair-fleet/v1` status receipts to content dashboard
  and repair-worker runs so operators can inspect the current single-runtime
  provider-owned repair coordinator/worker policy, ledger-based due scheduling,
  task pressure, and explicit non-production external-fleet/settlement posture.
- Added `elastos.content.network-abuse-policy/v1` status receipts to content
  dashboard, per-CID status, and repair-worker runs so operators can inspect
  provider-owned local guardrails, Carrier candidate caps, admission preflight
  posture, repair-worker budgets, configured abuse-control endpoint-quorum
  exchange posture, and explicit non-production network-wide
  throttles/banlists/abuse-ledger posture.
- Added `elastos.content.operator-dashboard/v1` to provider-wide content status
  so operators can inspect provider-local storage pressure, top principals,
  replica-byte estimates, quota-exceeded records, fleet-history attempts, recent
  repair rows, live-proof counts, and explicit non-production federation posture.
- Added `elastos.carrier.peer-reputation/v1` policy/status metadata to Carrier
  peer selection and content status so local Runtime success/failure scoring is
  visible while signed cross-runtime reputation remains explicitly not
  configured.
- Added `elastos.carrier.peer-attestation-exchange-policy/v1` metadata to
  Carrier peer selection, redacted remote receipt summaries, content proof
  summaries, and operator dashboard surfaces so signed availability
  announcements, verified remote content receipts, remote provider proofs, and
  local Runtime reputation are distinguished from unconfigured signed
  cross-runtime reputation receipts, third-party attestations, trust-policy
  exchange, and revocation.
- Added opt-in Carrier peer-attestation exchange: when
  `ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_*` is configured,
  `carrier-availability` posts a signed
  `elastos.carrier.peer-attestation.exchange-request/v1` with redacted remote
  proof summaries to one operator-owned exchange endpoint or a bounded endpoint
  set with an explicit quorum, requires accepted endpoint responses to include
  signed `elastos.carrier.peer-attestation.exchange-receipt/v1` receipts,
  verifies receipts before marking configured quorum accepted, records endpoint
  receipts plus quorum counters, and keeps connect tickets plus endpoint
  credentials out of app-visible proof surfaces.
- Added `elastos.content.storage-settlement-policy/v1` metadata to local,
  Carrier, ledger, per-CID, and provider-wide storage-market status so pricing,
  escrow, payment settlement, SLA enforcement, storage-market admission, and
  cross-provider escrow are explicit non-production policy state.
- Added `elastos.content.storage-market-admission-policy/v1` metadata to local,
  Carrier, ledger, per-CID, provider-wide, and operator-dashboard
  storage-market surfaces so local quota admission and remote
  `content/admission` preflight are distinguished from unconfigured production
  provider-admission networks, offer receipts, price discovery, SLA admission,
  and economic abuse controls.
- Preserved proof metadata through the standalone `availability-provider`
  capsule so configured external availability targets can report
  `storage_market`, `repair_graph`, and `abuse_controls` on the same
  Runtime-validated contract as built-in Carrier availability, with explicit
  no-market / target-report-only defaults when a target omits them; configured
  target fanout can now satisfy min-replica/live-proof requirements without
  bypassing max-replica quota.
- Added `elastos.content.federated-quota-ledger-policy/v1` metadata to local,
  principal storage-quota, Carrier quota, remote receipt, per-CID, provider-wide
  status, and operator dashboard surfaces so local per-principal ledgers and
  signed remote admission receipt exchange are distinguished from configured
  signed federated quota-ledger endpoint-quorum exchange and production
  quota-receipt exchange.
- Added opt-in federated quota-ledger exchange for remote admission preflight:
  `content/admission` can post a signed
  `elastos.content.federated-quota-ledger.exchange-request/v1` to one
  operator-configured endpoint or a bounded endpoint set with an explicit
  quorum, require signed
  `elastos.content.federated-quota-ledger.exchange-receipt/v1` receipts for
  accepted endpoints, record endpoint receipts and quorum counters in the signed
  admission receipt, and reject fail-closed on configured quorum failure,
  malformed signed receipt, timeout, or transport failure without exposing
  endpoint credentials.
- Added opt-in federated abuse-control exchange for remote admission preflight:
  `content/admission` can post a signed
  `elastos.content.federated-abuse-control.exchange-request/v1` to one
  operator-configured endpoint or a bounded endpoint set with an explicit
  quorum before quota-ledger, storage-market, byte-transfer, or repair-graph
  movement, require signed
  `elastos.content.federated-abuse-control.exchange-receipt/v1` receipts for
  accepted endpoints, record endpoint receipts and quorum counters in the signed
  admission receipt, and reject fail-closed on configured quorum failure,
  malformed signed receipt, timeout, transport failure, or receipt verification
  failure without exposing endpoint credentials.
- Added `elastos.content.external-repair-fleet-policy/v1` metadata to
  provider-wide status, repair-worker runs, and operator dashboard surfaces so
  the local provider-owned repair worker is distinguished from unconfigured
  external coordinators, volunteer/supernode workers, cross-provider queues,
  worker attestations, settlement, and repair SLAs.
- Added `elastos.content.federated-operator-alerting-policy/v1` metadata to
  provider-wide status and operator dashboard surfaces so provider-local status
  JSON, storage pressure, repair-task pressure, live-proof counters, and
  remote-receipt counters plus configured provider-local alert sink and
  configured federated alert-exchange posture are distinguished from production
  cross-provider dashboards, peer-health subscriptions, fleet-wide SLA policy,
  and operator UI.
- Added opt-in operator alert delivery for content availability: provider-wide
  `content/status` can emit a durable
  `elastos.content.operator-alert.receipt/v1` outbox entry, post an
  `elastos.content.operator-alert/v1` payload to one HTTPS or loopback sink
  when `ELASTOS_CONTENT_OPERATOR_ALERT_*` is configured, and deliver a typed
  `elastos.content.federated-operator-alert.exchange-request/v1` to one
  operator-owned exchange endpoint when
  `ELASTOS_CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_*` is configured. The
  normalized
  `elastos.content.federated-operator-alert.exchange-receipt/v1` is recorded
  beside the provider-local sink result without exposing operator credentials to
  apps or status JSON.
- Added runtime authority primitives for proof-bound authentication: principals, proof bindings, SIWE challenges, session grants, and audit events.
- Added EVM SIWE challenge, verify, and revoke gateway routes that bind verified wallet proofs to runtime principals and issue scoped Home/System launch grants.
- Added `chain-provider` as the first blockchain provider capsule: typed `elastos://chain/*` access for Essentials-compatible Elastos networks without exposing raw RPC URLs to app capsules.
- Added wallet approval approve/complete states, Inbox review entries, WebConnect-style handoff metadata, and signature receipt hashes without exposing wallet signatures to app capsules.
- Added passkey-controlled built-in EVM wallet creation through Wallet, with provider-owned encrypted key storage and managed signing only after Wallet/Inbox review plus fresh passkey confirmation in Wallet.
- Added fresh passkey-bound gates for built-in wallet signatures, Wallet send, account deletion, and Wallet recovery-key export/import so long-lived app launch tokens cannot execute managed-wallet authority by themselves.
- Bound managed-wallet private-key envelopes to principal/account/chain/address metadata using AES-GCM AAD and principal-derived storage keys, with tamper tests for cross-principal/chain metadata changes.
- Removed internal wallet storage object paths from wallet-provider init/status responses; consumers now see only configured booleans and counts.
- Hardened `wallet-provider` request parsing so hidden signing authority, connector wallet objects, and extra wallet fields fail at decode time.
- Added a dedicated `wallet-metamask` connector capsule for MetaMask SIWE linking and external wallet approval completion, keeping browser wallet authority out of System.
- Added connector-bound external wallet links and approval completion so EVM wallet proofs and signatures can only be finished by the dedicated connector capsule that owns the account.
- Generalized external wallet connector approval/account routes behind an allowlisted connector-capsule path, keeping MetaMask working while preventing unknown connector capsules from receiving wallet handoffs.
- Added PC2 convergence documentation and provider-resource tests that translate PC2 wallet bridge method classes into Runtime wallet/chain scopes while rejecting raw EIP-1193 methods as provider operations.
- Added typed chain proof, EVM transaction prepare, signed transaction broadcast, and node lifecycle status scopes to `chain-provider`.
- Hardened `chain-provider` request parsing so hidden raw transaction and node RPC fields fail at decode time before provider logic runs.
- Added signable EIP-155 legacy transaction intents from `chain-provider` and built-in wallet transaction signing after Runtime approval, while external wallet transaction signing remains connector-bound.
- Added ERC-1271 smart-account SIWE proof verification through a typed `chain-provider` proof followed by wallet-provider challenge consumption, keeping smart-account proof checks out of app capsules.
- Added typed Bitcoin BIP-322 simple proof challenge/verification for Bitcoin mainnet native P2WPKH addresses, with wrong-message and unsupported-script paths failing closed behind `elastos://wallet/proof/bip322/*`.
- Added connector-token-scoped BTC wallet-link routes for BIP-322 challenge/verify so a connector capsule can bind a Bitcoin proof to the existing passkey principal without minting a Home session or exposing raw wallet/node authority.
- Added manual BIP-322 proof-link handoff inside the visible `wallet` capsule, keeping MetaMask EVM-only until a documented Bitcoin dapp signing API exists.
- Added a dormant `wallet-walletconnect` connector capsule UI that stays hidden/unroutable until operator-pinned WalletConnect config and a local Reown/AppKit adapter hash are present.
- Added a WalletConnect connector config utility and smoke proof that copy a local reviewed adapter into the runtime data dir and pin its sha256 before the connector can launch.
- Added an exact-version WalletConnect adapter build script for producing the local Reown/AppKit adapter bundle used by the connector gate.
- Added an entropy guard proving WalletConnect requires an explicit operator Project ID and local SDK hash pinning, with no repository or environment default Project ID.
- Added a visible Wallet surface that replaces the old Bitcoin-first wallet app with accounts, native ESC/Base/BTC balance reads through `chain-provider`, default-account selection, approval review, and connector handoffs as approval methods.
- Added the first visible `browser` capsule shell with explicit `elastos://wallet/*` and `elastos://net/*` capability intent, a Glide default URL for testing, and honest cross-origin wallet-injection boundaries until the native/webview or microVM Browser/Net/Exit adapter exists.
- Added `net-provider` as the first fail-closed Browser/Net boundary: it validates Browser requests, blocks LAN/private targets, rejects hidden raw authority fields, and returns an explicit `exit_unavailable` handoff instead of touching host networking itself.
- Added `exit-provider` as the internal Browser egress contract with fail-closed `quote`, `open_stream`, `close_stream`, and `http_fetch` operations for future local, Carrier-routed, privacy, paid, or enterprise exit backends.
- Added the first constrained Browser egress proofs: `/api/provider/net/http` and `/api/provider/net/stream` now validate through `net-provider` and then hand off internally to operator-configured `exit-provider` `http_fetch` or `stream_relay` backends with host allowlists, body limits where bytes are returned, and private-target blocking by default.
- Moved the visible Browser preview request path from HTTP fetch to stream-session reservation so the UI exercises the intended browser path and reports `byte_transport: not_attached` until a Browser Engine Adapter exists.
- Added `browser-engine-adapter` as the internal Browser Engine Adapter contract with fail-closed `status`, `launch`, `attach_stream`, and `close_page` operations; it requires explicit operator config and attached `adapter_ipc` byte transport before any page launch can succeed.
- Added `/api/apps/browser/open` as the high-level Browser product route that performs Runtime-owned Net validation, Exit stream reservation, and Browser Engine Adapter launch without exposing raw Exit or Browser Engine provider routes to ordinary capsules.
- Added Browser Session Manager proof surfaces: launch reservations, per-principal/total capacity receipts, page heartbeats, stale active-page cleanup, and gateway-level smokes that prove concurrent Home-launched Browser page accounting closes without leaving capacity behind.
- Added a protected-content Browser reachability smoke for the known `ela.city` item route. This is intentionally scoped to route/session cleanup and does not claim purchase, key release, decrypt, or playback readiness.
- Added typed `elastos.adapter-ipc/v1` descriptors for configured Browser stream backends and stripped those private endpoint descriptors from Browser UI responses while passing them to the internal Browser Engine Adapter.
- Added typed `elastos.exit.relay-ipc/v1` descriptors for private Exit relay sockets; Gateway uses them only internally to relay bytes from the Runtime-owned Browser stream socket to an operator/Carrier exit daemon, strips them from Browser UI, and never passes them to the Browser Engine Adapter.
- Added the Browser Engine native supervisor handshake: Runtime sends `elastos.browser.engine.launch-request/v1` through `ELASTOS_BROWSER_ENGINE_REQUEST`, and native adapters only launch when the supervisor returns a validated `elastos.browser.engine.supervisor-result/v1` with runtime-net-only, no-direct-network, and no-wallet-injection proofs.
- Added `browser-engine-supervisor` as the first Linux host helper for native browser engines: it validates operator config, starts the configured engine under `linux_new_netns`, and passes only stream/IPC/target/URL environment to the child process.
- Added `browser-stream-bridge` as the first Linux local byte-transport helper for Browser Engine work: it forwards between a private engine Unix socket and a Runtime-owned Unix stream socket without TCP, DNS, HTTP, wallet, chain, or raw host-network authority; the supervisor can launch it before the native engine through `ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG`.
- Added Runtime-owned Browser stream socket path allocation: Gateway injects a private `Runtime/BrowserStreams/*.sock` path into the internal Browser Engine launch descriptor, binds it as a local Unix listener, closes fail-closed when no private Exit relay exists, and keeps `adapter_ipc`/`relay_ipc` hidden from Browser UI responses.
- Added `browser-local-exit` as the first server-side Browser Exit relay: Runtime sends a typed `elastos.exit.relay-open/v1` handshake to its private Unix socket, and the helper dials only operator-allowlisted public targets while blocking private resolved IPs by default.
- Changed Home launch so the Browser capsule opens as an ElastOS window instead of escaping into a host browser tab.
- Added persistent typed node-lifecycle state to `chain-provider`, including reload coverage that does not persist or return raw node RPC URLs.
- Added proof-bound Recovery Kit export/import request routes that validate principal/root binding, require active Home/System authority, and record signed audit denials.
- Added validated principal-root protection state so recovery status can report verified recovery protection without allowing cross-principal protection records to bleed across users.
- Added proof-bound Recovery Kit creation/import/export: the runtime generates a per-principal data key, wraps it to a recovery phrase, stores a runtime-encrypted downloadable archive plus protector metadata, and verifies encrypted root descriptors on import.
- Added optional password-packaged Recovery Kit downloads and password-verified package import so users can protect the offline kit file without giving apps raw recovery authority.
- Added public route-level Recovery Kit coverage for create, status, password-packaged export, wrong-password rejection, and verified import through `/api/auth/recovery/*`.
- Added contract-level WebAuthn PRF recovery-protector validation so PRF protectors must use `webauthn-prf` envelopes and cannot carry Recovery Kit archives.
- Hardened runtime WebAuthn response parsing so hidden extension payloads such as `clientExtensionResults` are rejected until client-side PRF wrapping exists.
- Added contract-level DID recovery-protector validation so DID protectors must identify a `did:key` or `did:elastos` subject, use DID-bound envelope metadata, and cannot masquerade as downloadable Recovery Kits.
- Hardened principal-root protection and Recovery Kit contracts so hidden or unknown nested fields fail during decode instead of being accepted and ignored.
- Added a typed `did-provider` recovery-proof verification operation for `did:key` subjects that binds the proof to principal, root, protector, data-key ID, nonce, and expiry.
- Hardened `did-provider` request parsing so hidden fields on typed DID recovery and chat-signing requests are rejected instead of being accepted and ignored.
- Hardened all `op`-tagged provider request contracts to reject unknown top-level fields, covering Documents, localhost, IPFS, availability, protected-content, AI, WebSpace, tunnel, and provider bridge operation envelopes.
- Hardened protected-content object and request contracts so sealed objects, key envelopes, key-release requests, decrypt-session requests, DRM open requests, rights checks, and availability ensure calls reject hidden nested authority fields at decode time.
- Hardened browser-facing gateway request bodies so Home state, System settings, wallet approvals, Home launches, Inbox actions, chat access, room messages, and upload starts reject hidden authority fields at decode time.
- Hardened capability API request bodies so request, grant, deny, revoke-all, and audit query inputs reject hidden authority fields at decode time.
- Wired Recovery Kit import to consume typed DID recovery proofs through `did-provider` only when they match an existing DID recovery protector on the recovered root, preserving that protector without claiming DID-only recovery.
- Added System smoke coverage for Recovery Kit password-protected download and import controls so the user-visible recovery path stays tested.
- Added Home smoke coverage for Recovery Kit controls inside the Home-launched System frame.
- Added `scripts/recovery-kit-live-smoke.sh` so signed browser sessions can prove live Recovery Kit status/download/import without silent mutation; it accepts a copied token, Cookie header, or cookie jar from the signed Home session.
- Fixed the live Recovery Kit smoke script to validate the current `elastos.principal.root-recovery.status/v1` schema before export/import.
- Added `scripts/auth-wallet-focus-smoke.sh` as a repeatable gate for the branch's passkey, recovery, recovery-protector contract, capsule-bridge principal storage, principal-launch, System managed-wallet route, wallet, BTC, typed chain proof/prepare/broadcast, chain sync-health, node lifecycle, entropy, and alignment checks.
- Extended the auth/wallet focus smoke with MetaMask connector and Wallet Bitcoin-proof journey filters.
- Added Home Chat Room launch coverage to the auth/wallet focus gate so Home-to-runtime launches must use signed `launch_grant` authority and never raw `principal_id`.
- Added explicit Recovery Kit root reassignment: System can import a verified kit for an existing principal root, rebind the active passkey to that root, reissue Home/System session tokens, restore included built-in Wallet keys, and record signed audit.
- Hardened Recovery Kit root reassignment so recovery replaces old passkey-root bindings, revokes their sessions, and public handler coverage proves the response rotates the Home session cookie after reassignment.
- Hardened System Recovery Kit import so recovering an account requires an explicit in-surface `Recover account` action instead of silent reassignment.
- Added runtime-enforced principal-root object encryption for protected roots: Documents working copies, Home browser state, and viewer/content storage now write AES-256-GCM envelopes bound to principal, root, data-key ID, and object URI, and reject plaintext reads for protected roots.
- Fixed protected-root Home loading so old plaintext browser-state files are treated as untrusted UI state and reset to a clean default instead of causing a 500 or being accepted as data.
- Hardened the legacy generic `/api/localhost/*` storage handler so `Users/*` roots fail closed without a runtime principal-scoped provider route.
- Updated the Home CLI `Users` root descriptor to show principal-root storage instead of the obsolete shared `Users/self` alias.
- Removed stale shared `Users/self` examples from generic capability/provider-resource tests so only principal-scoped bridge paths keep that alias.
- Removed the stale shared `Users/self` path from VM provider path-shaping tests.
- Added an entropy guard that only allows `Users/self` in approved scoped-alias code, capsule manifests, and regression tests.
- Renamed the System account display field from Handle to Name so local profile copy is not confused with global name-claim semantics.
- Added `scripts/protected-home-state-smoke.sh` to prove the protected Home browser-state reset regression and live unsigned Home summary path before handing browser-visible changes back for testing.
- Added passkey revocation policy coverage proving admins can revoke guest passkeys without revoking their own session while guest-to-admin and last-admin removal still fail closed.
- Clarified WebAuthn RP authority handling so Host-derived same-origin requests are not described as a fallback around malformed browser origins.
- Added admin-controlled guest passkey promotion through a runtime auth route and System action, with guest self-promotion rejected.
- Added admin-controlled admin passkey demotion through a runtime auth route and System action, while rejecting self-demotion and last-admin demotion.
- Removed the unsigned Home unlock copy flicker so the passkey card starts with the final sign-in message instead of a temporary checking message.
- Simplified Home sign-in so the browser passkey chooser opens automatically, sign-in no longer shows the passkey-name field or leaked form label, and guest creation is a separate flow.
- Fixed Home passkey cancellation so dismissed browser passkey prompts show a clean `No passkey selected` state with `Create guest account` still available.
- Fixed Home/System appearance state so wallpaper and overlay preferences are stored under the active passkey principal root instead of a shared runtime bucket.
- Clarified Wallet UI language around Accounts and Approval methods so MetaMask, Bitcoin, WalletConnect, and passkey-managed signing read as connector-backed approval paths under one Wallet model instead of separate wallet products.
- Cleaned Wallet Approval Methods so linked accounts are removable from Wallet after fresh passkey confirmation, MetaMask can add another account, WalletConnect remains disabled without pinned operator config, Ledger stays hidden until implemented, and UniSat is not advertised as available from hosted Browser environments.
- Added principal-scoped WASM launch plumbing: Home-backed runtime launches forward the signed launch-token principal into WASM bridge pipes so capsule-facing `localhost://Users/self` can resolve through the runtime principal root instead of a shared alias.
- Routed protected in-runtime capsule-kernel `localhost://Users/self` read/write calls through the runtime principal-root object envelope when a Home launch principal is present, and made attached/remote bridge user-root storage fail closed until it has the same protected bridge, preventing protected user-root state from falling through to generic localhost-provider storage.
- Added route-level rejection tests for `/api/capsules` raw principal injection so supplied `principal_id` values fail closed before reaching the runtime bridge.
- Replaced raw `/api/capsules` principal injection from Home with a signed, app-scoped `launch_grant`; raw `principal_id` launch authority is now rejected even when it is paired with a grant.
- Tightened managed Home/chat runtime policy so capability grants target principal-root storage, while the capsule bridge rejects explicit foreign `localhost://Users/<root>` requests before approval prompts.
- Fixed managed runtime-backed Home launches so Chat Room and other app capsules can validate signed principal launch grants instead of failing with `principal launch grant unavailable`.
- Hardened the shell/supervisor microVM launch path so it accepts the same signed app-scoped `launch_grant`, rejects raw `principal_id`/`home_token` authority and authority-shaped config, passes the verified principal into the microVM Carrier bridge, and refuses provider-role user scope.
- Added System-gateway coverage for `chain-provider` node lifecycle status so lifecycle checks use the same launch-token and capability-resource path as other System chain diagnostics.
- Added operator-approved loopback node supervisor control to `chain-provider` for start/stop/restart without returning raw node URLs, ports, command paths, or process handles.
- Added System node lifecycle controls that appear only when `chain-provider` reports `control_available=true`; remote/public RPC networks stay status-only.
- Added wallet approval journey coverage: provider-created typed signature requests appear in Wallet/Inbox, approval executes managed signing, and completion records signed audit without exposing wallet authority to apps.
- Added provider-backed default wallet routing: System selects the principal's default account, and typed signature requests must name `chain_namespace + intent` before the wallet-provider resolves a default or verifies an explicit same-chain account.
- Added anti-drift checks that block ordinary app/viewer/content capsules from referencing raw wallet, chain, node, RPC, WalletConnect, MetaMask, or blockchain-provider authority directly.
- Added shared protected-content schemas and a fail-closed `drm-provider` contract for `elastos://drm/meta/status` and `elastos://drm/open`.
- Added `rights-provider` as the typed, fail-closed protected-content policy boundary for access, subscription, stream, and download questions.
- Added `key-provider` as the typed, fail-closed protected-content key-release boundary for PQ-hybrid dKMS requests.
- Added `decrypt-provider` as the typed, fail-closed protected-content decrypt/render session boundary.
- Added a canonical `drm-provider.status.required_sequence` for protected-content open orchestration before backend wiring.
- Added Runtime-owned release receipt and audit steps to the protected-content open sequence.
- Added the same machine-readable required sequence and runtime events to fail-closed `drm-provider.open` responses.
- Added `scripts/protected-content-provider-contract-smoke.sh` to exercise protected-content provider capsules through their real JSON line protocol.
- Added `scripts/installed-provider-verify.sh` so installed provider binaries can be checked against the installed `components.json` before live browser testing.
- Added alignment checks and release-story documentation so the protected-content provider journey proof stays visible in `TASKS.md`, `state.md`, and the runtime repo checklist.
- Added algorithm metadata to protected-content key envelopes so sealed objects can declare cipher, signature, KEM, and share-scheme choices for PQ-hybrid dKMS work.
- Enforced protected-content key-envelope algorithm allowlists for AES-256/ChaCha20 payload encryption, hybrid X25519 + ML-KEM share wrapping, and classical + PQ signatures.

### Changed
- Upgraded Home launch tokens to carry principal, session, proof-binding, grant, expiry, and non-delegation context, with active-session validation for proof-bound tokens.
- Aligned principles, roadmap, capsule model, and Carrier docs around the local-and-off-box Carrier plane and blockchain quadrant authority model.
- Added an explicit `blockchain` setup profile and release/build support for `chain-provider` while keeping it out of the default Home profile.
- Enforced carrier-only authority for ordinary app/viewer/content capsule manifests and added repo-wide alignment checks for forbidden manifest authority.
- Moved the headless agent capsule from direct runtime HTTP calls to the guest Carrier-kernel SDK and added an alignment check against direct host-route usage in ordinary Rust/WASM app capsules.
- Made the capsule bridge reject raw runtime-control requests with an explicit `not_capsule_kernel_abi` error so app capsules stay on capability/`carrier_invoke` calls.
- Removed raw shell/runtime-control, direct storage, direct provider routing, and direct capsule-message helpers from the guest SDK so capsule code only sees capability requests and `carrier_invoke`.
- Moved first-party chat, agent, and Home CLI capsules from `provider_call(scheme, op)` to the URI-based `carrier_invoke(uri, operation)` capsule-kernel ABI.
- Documented the older `elastos-runtime` handler as an internal shell/control protocol, not the public guest SDK, and added a test that `carrier_invoke` stays on the Carrier bridge.
- Added browser-host-adapter proof that attached WASM capsules cannot use HTTP bridge routes for raw runtime control.
- Required provider capsule manifests to declare their owned `provides` namespace, including the WebSpaces resolver.
- Required provider capsule manifests to declare provider-authority metadata with reason strings, capability schemas, operation lists, and expected audit events.
- Added a built-in `content` provider seam above `ipfs-provider` and moved Documents publish/unpublish onto that availability contract with honest local availability status.
- Made `elastos://ipfs/*` a system-only backend at the capability request surface so ordinary capsules must use `elastos://content/*`.
- Added signed local availability receipts for content publish/unpublish and exposed the latest receipt through `elastos://content/status`.
- Moved site publish/activate CID creation onto the `elastos://content/*` path while keeping `ipfs-provider` as the low-level backend.
- Moved `elastos share`, `elastos shares` channel-head updates, and `elastos attest` provenance writes onto the `elastos://content/*` path.
- Added `elastos://content/fetch` for CID/path reads and moved provenance verify/read helpers onto the content provider path.
- Routed `/s/<cid>/...` gateway file reads through `elastos://content/fetch` instead of raw `ipfs-provider` requests.
- Moved share metadata and channel-head reads onto `elastos://content/fetch`.
- Rejected ordinary app/viewer/content manifest capabilities for system-only backend namespaces such as `elastos://ipfs/*`, Kubo, IPFS Cluster, Elacity SDK, and runtime SystemServices storage.
- Added `elastos://content/repair` so the content provider can re-pin a CID locally or record a signed `repair_needed` receipt.
- Added `elastos://content/ensure` as the idempotent availability operation and made content status reject invalid CIDs instead of returning ambiguous unknown state.
- Added a registered availability-provider seam so content publish/ensure can return `network_available` or `repair_needed` when a runtime-owned replication provider verifies network availability.
- Added an `availability-provider` capsule that forwards `elastos://content/ensure` availability requests to explicitly configured Elacity/supernode-compatible targets without hardcoded public service assumptions.
- Removed stale raw-IPFS helper paths from the main command dispatcher and added an alignment guard so command materialization stays on `elastos://content/*`.
- Added deterministic `_elastos_object.json` manifests to directory publishes, giving Documents, shares, and sites a common IPLD-compatible object shape.
- Extended content object manifests with deterministic CID links plus `release` and `sealed` object kinds for release manifests and protected-content descriptors.
- Made `sealed` content object publishes fail closed unless they include `sealed.json`, payload/rights/availability/provenance links, and approved protected-content key-envelope algorithms.
- Made content directory publishes sort package entries and reject duplicate paths or unknown object kinds before bytes reach the IPFS backend.
- Made Documents unpublish receipts preserve the document owner DID instead of falling back to the runtime device DID.
- Tightened ordinary capsule manifests so apps, viewers, and content cannot declare external host dependencies, provider implementation overrides, or microVM HTTP ports.
- Unified provider capability-resource derivation across the HTTP host adapter and Carrier bridge so local and capsule-kernel calls fail closed against the same resource contract.
- Removed duplicate wallet-provider transaction prepare/broadcast declarations; typed transaction prepare/broadcast belongs to `chain-provider`, while `wallet-provider` owns approval and signing receipts.
- Narrowed `elastos://content/*` capability-resource derivation to documented publish/fetch/status/ensure/repair/unpublish operations instead of a broad content wildcard.
- Made capsule-kernel capability requests fail closed for unsupported schemes and system-only backends such as raw gateway/IPFS/Kubo/Elacity namespaces before they can create user approval prompts.
- Routed ordinary `elastos publish` capsule directory uploads through `elastos://content/*`, while leaving large MicroVM rootfs streaming on the existing explicit operator path.
- Routed `elastos open elastos://<cid>` data-capsule materialization through `elastos://content/*` and verified `_elastos_object.json` file size/hash metadata before serving.
- Routed `elastos run --cid` and `elastos serve --cid` materialization through `elastos://content/*` instead of the raw IPFS bridge.
- Routed supervisor-installed capsule artifact downloads through `elastos://content/fetch` instead of direct `ipfs` sub-provider calls.
- Routed public gateway installer publishing through `elastos://content/publish` instead of direct `ipfs` sub-provider calls.
- Added an operator `elastos content publish-object` path and release-object sidecars so public releases can expose IPLD-compatible manifest links without breaking raw installer CIDs.
- Taught release/update bookkeeping to validate and display signed `release_object_cid` values while preserving raw `latest_release_cid` installer compatibility.
- Taught `elastos open elastos://<release-object-cid>` to open release objects as verified metadata summaries and made CID materialization reject release objects as non-launchable content graphs.
- Added alignment gates so CID run/serve and public gateway publish paths cannot reintroduce raw IPFS materialization.
- Tightened passkey/WebAuthn ceremonies to require user verification in browser options and reject authenticator data without the user-verification flag.
- Added a first-class `passkey_webauthn` runtime proof-binding model so passkeys can bind principals without becoming wallet or DID replacements.
- Changed successful WebAuthn registration/authentication to return verified credential facts for runtime proof-binding issuance.
- Bound successful passkey registration/authentication into runtime auth state by upserting a passkey proof binding and issuing a short-lived Home/System session grant.
- Clarified WebAuthn RP/origin derivation so localhost development uses `http://localhost` while hosted Home uses its HTTPS origin.
- Added browser-gateway passkey register/sign-in endpoints and System passkey controls that issue Home/System launch grants without wallet-first UX.
- Promoted passkeys into the default Home entry path so fresh browsers see a Home unlock surface instead of receiving an automatic local session cookie; successful passkey registration/sign-in sets the same refresh-safe HttpOnly Home session cookie.
- Made passkey the Home front door authority: the first passkey on a runtime becomes admin, later passkeys become guest principals with their own `localhost://Users/<principal-root>` area, guest creation defaults off, and System admin controls new guest enrollment without revoking existing guests.
- Made guest passkey creation nameable and same-authenticator friendly by omitting `excludeCredentials` for new runtime principals while preserving duplicate-prevention for legacy backup-passkey registration.
- Made first Home passkey creation nameable, derived the visible handle from the active passkey principal, and documented orphaned user-root recovery semantics.
- Removed the proof-less System handle path, scoped guest passkey lists to the current guest principal, and made passkey revocation runtime-enforced so guests cannot remove admin passkeys.
- Removed admin-created guest passkeys from System; admins now only open or close guest enrollment, while guests self-register their own passkey and principal from Home when enrollment is enabled.
- Added principal-root protection and Recovery Kit contracts plus a proof-bound recovery status route that stays honest about unencrypted or unprotected roots.
- Scoped viewer/content storage such as GBA save states through the signed Home launch-token principal instead of writing under a shared literal `localhost://Users/self` directory.
- Scoped Documents provider working copies through the signed Home launch-token principal and rejected cross-principal document operations at the provider boundary.
- Removed the global notification-to-native-chat relay that wrote room events into shared `localhost://Users/self` state without an active principal.
- Made the generic capsule-kernel bridge map `localhost://Users/self` through an explicit principal context, while rejecting capability requests and carrier invokes when that context is missing.
- Made unsigned Home load as a standard non-user desktop that prompts for passkey sign-in while keeping runtime ensure, app launch, and browser state writes capability-gated.
- Aligned the Home passkey prompt with the PC2 login surface: centered dark card, ElastOS branding, amber action, and concise passkey copy without wallet-first dependencies.
- Simplified Home sign-in copy around data, apps, and desktop access and tightened toolbar spacing so the sign-out control remains fully visible.
- Simplified System into Account, Appearance, and collapsed Advanced areas so routine passkey, guest, and wallpaper settings are not mixed with runtime diagnostics.
- Renamed the visible System Advanced DID field to Device identity and clarified that passkey principals, `did:key`, `did:elastos`/EID, handles, CIDs, and IPLD object graphs are separate identity layers.
- Signed runtime audit events with the runtime DID key before persisting them in auth state.
- Kept the Home passkey card stable through status checking and signed boot so registration/sign-in does not flash through intermediate desktop states.
- Wired Home to refresh proof-bound passkey sessions through the runtime session-refresh route after signed boot and during long-lived desktops.
- Fixed proof-bound session refresh to accept the browser's HttpOnly `home-session` cookie, preventing successful passkey sign-in from falling back into the unlock prompt.
- Added explicit Home sign-out that revokes the current proof-bound session grant, clears the HttpOnly `home-session` cookie, and reloads into the unsigned passkey prompt.
- Bound Home open-window session restore to a browser-context id in site storage and de-duped restored targets so clearing browser site data cannot replay stale server-side System windows after sign-in.
- Moved Home browser layout/session/recent-target state into the active principal's `localhost://Users/<principal-root>/.AppData/ElastOS/Home/` area instead of a shared system bucket.
- Made browser-gateway passkey routes load identity state lazily and fail closed without creating identity material during unrelated reads.
- Made WebAuthn RP derivation fail closed for malformed or insecure browser origins and documented hosted Home, localhost, PWA, and future WebView adapter boundaries.
- Added proof-bound passkey list, credential revoke, and session-refresh routes for Home/System without exposing passkey controls to app capsules.
- Expanded fail-closed passkey coverage for replayed or expired challenges, wrong origin/RP, missing user verification, counter regression, missing grants, and revoked proof bindings.
- Bound multiple passkeys for the same local identity to one runtime principal and required an existing Home/System grant before adding backup passkeys.
- Replaced capsule-facing arbitrary DID `sign(data)` with a typed chat-message signing intent so the DID provider no longer exposes generic private-key signing to app surfaces.
- Defined the wallet-provider contract and expanded the chain-provider capability schema before adding blockchain write/broadcast UI or provider behavior.
- Added wallet-provider capability-resource mapping so future `elastos://wallet/*` calls are scoped and fail closed instead of falling through to broad provider wildcards.
- Added the first `wallet-provider` capsule slice for linked-account metadata under runtime-managed storage, with proof, signing, and transaction operations failing closed until approval/proof enforcement exists.
- Extracted shared auth primitives into `elastos-auth` and added wallet-provider SIWE challenge/verify support with single-use proof challenges while keeping signing and transactions fail-closed.
- Routed browser EVM wallet login through `wallet-provider` proof challenge/verify and linked verified accounts through the provider before Runtime issues scoped Home/System grants.
- Added wallet-provider typed signing approval requests so `request_signature` records pending, principal-scoped approval state instead of exposing arbitrary signing or wallet RPC.
- Added System wallet approval review/reject APIs and UI so pending typed wallet requests can be inspected without giving apps direct wallet authority.
- Added a typed `chain-provider` rights-read seam for `has_access_by_content_id` that validates protected-content inputs and fails closed until rights ABI configuration exists.
- Added configurable `chain-provider` rights-method ABI support for `hasAccessByContentId(string,address,string) -> bool`, scoped to approved contracts/selectors and still without arbitrary RPC passthrough.

### Removed
- Removed the generic `chain-provider` `call` operation so chain access stays on reviewed typed operations instead of arbitrary `eth_call` inputs.

### Fixed
- Hardened EVM auth challenges to derive origin from the runtime request, reject client-supplied origin fields, and verify the exact runtime-issued SIWE message.
- Made source-checkout `elastos setup --list` use the checkout `components.json` before any stale installed manifest, so developer runs show current Home/blockchain profiles.
- Rejected path-like document DIDs before joining document metadata paths.
- Narrowed viewer-bound object launch tokens so a token for one content capsule cannot enumerate the full viewer library.
- Required recovery status to distinguish verified recovery protectors from merely present root-protection metadata.
- Required approved wallet requests to expire before managed signing or external wallet completion can execute.
- Updated managed-wallet namespace errors so Bitcoin-capable wallet creation no longer reports stale EVM-only guidance.
- Rendered System passkey admin actions only after the runtime access role is loaded, so admins can promote guest passkeys without reloading System.
- Persisted an explicit empty Home browser-window session after the last window closes, preventing stale windows from reopening after refresh.

## [0.2.0] - 2026-04-29

### Added
- Added the Home browser shell capsule, `home-cli`, and the `elastos home` command path as the visible front door.
- Added first-party System, Inbox, Library, Documents, Chat Room, GBA Emulator, and uCity browser/content capsules to the shipped Home catalog.
- Added runtime-owned browser capsule routing, object/viewer launch foundations, and app-scoped launch tokens for Home-launched surfaces.
- Added Documents provider APIs for summary, create, get, save, save-as, publish, unpublish, delete, and immutable `elastos://<cid>` document opens.
- Added Library document browsing and Chat Room attachment flow through Home orchestration instead of browser file upload.
- Added Chat Room browser-session pairing, same-browser Home session reuse, guest identity separation, guest kick controls, and runtime member invite/block controls.
- Added System appearance controls for wallpaper, reset, overlay toggle, and overlay opacity.
- Added Home PWA metadata, mobile fullscreen support, touch-first desktop icon behavior, reversible desktop icons, and mobile-safe window behavior.
- Added GBA save-state persistence, mobile touch controls, keyboard mapping labels, fullscreen ratio handling, compact controls, and fail-fast unsupported-WebView detection.
- Added shared first-party design-system docs and `scripts/home-entropy-check.mjs` for UI, naming, authority, and stale-copy drift checks.

### Changed
- Renamed the visible product front door from PC2 to Home and aligned setup profiles, proof scripts, docs, and CLI smoke tests around the Home naming.
- Replaced `md-viewer` with Documents and `room-browser` with Chat Room.
- Split setup profiles more explicitly between the core Home path, the broader demo surface, and the explicit operator lane.
- Hardened release proofing around clean-home setup, the PTY Home front door, `chat-room` packaging, source-local trusted-source checks, and Home/browser journey smoke coverage.
- Moved Home wallpaper and contrast overlay configuration into `System -> Appearance` backed by the runtime appearance store.
- Made Documents object-first: DID-backed document identity is the mutable object, `localhost://Users/self/Documents/...` is only the local working copy, and `elastos://<cid>` is the immutable published revision.
- Made Library content-first around documents and typed content instead of raw working-copy paths.
- Moved Inbox list rendering, read state, approval, denial, dismissal, and source-app open actions into the Inbox capsule; Home now owns only badge and launch.
- Aligned first-party capsule UI colors, spacing, mobile padding, and accessible controls with the shared light capsule token set.
- Aligned roadmap, principles, architecture, namespaces, and security docs around the four quadrants, object/capsule/space ontology, and capability-scoped Carrier/provider boundary.

### Fixed
- Unified main DID derivation with the device key and aligned local nickname persistence onto one shared codec.
- Removed stale live-host conflicts so managed Home/chat lanes and the explicit operator lane do not silently share one home.
- Cleaned up the public room naming so the shipped `chat-room` route, packaging, and proof tooling all agree.
- Moved Documents publish/unpublish IPFS-specific logic out of the gateway edge into the provider plane.
- Removed the gateway-owned IPFS provider bridge path; public CID reads now use cached content or the runtime provider registry and fail closed otherwise.
- Required Home authority before minting app launch tokens, app-scoped launch tokens for System and Inbox APIs, and browser-context-bound chat-room access polling.
- Redacted room bearer tokens from public/Home summaries and preferred the native Home room session over paired browser identity when both exist in one browser profile.
- Fixed browser Chat Room identity handling so messages from the native Home member no longer appear as the browser guest's own messages.
- Fixed Documents publish/unpublish behavior so unchanged content does not produce unnecessary new published revisions.
- Fixed document delete confirmation to use in-surface UI instead of browser alerts.
- Fixed Home window dragging so windows can move partially offscreen without jumping back when their title bar is clicked.
- Fixed desktop drag selection, desktop icon removal/re-add, mobile launcher focus, and maximized-window coverage over Home chrome.

## [0.1.2] - 2026-04-16

### Added
- Added device-backed local identity profile storage and shared DID-backed nickname handling across the CLI, did-provider, and PC2 surfaces.
- Added hosted browser-capsule foundation, the shipped `room-browser` asset set, and sovereign room invite/accept control with cross-runtime Carrier sync.
- Added explicit operator-lane setup, remote node control over Carrier, and release-line public-install/operator acceptance scripts.

### Changed
- Kept PC2 as the honest front door by surfacing room, chat, and identity flows with the current runtime and return-home contract.
- Split setup profiles more explicitly between the core PC2 path, the broader demo surface, and the explicit operator lane.
- Hardened release proofing around clean-home setup, the PTY PC2 front door, room-browser packaging, and source-local trusted-source checks.

### Fixed
- Unified main DID derivation with the device key and aligned local nickname persistence onto one shared codec.
- Removed stale live-host conflicts so managed PC2/chat lanes and the explicit operator lane do not silently share one home.
- Cleaned up the public naming around `room-browser` so the shipped browser route, packaging, and proof tooling all agree.

## [0.1.1] - 2026-03-31

### Fixed
- Removed the installer's undeclared `xxd` dependency from signature verification so minimal environments can install from the canonical gateway without extra packages.
- Pinned the documented and declared Rust toolchain to `1.89+` so fresh source builds match the actual compiler floor.
- Tightened PC2 home guidance and native chat runtime reuse so the public onboarding path stays coherent on WSL and Jetson.

## [0.1.0] - 2026-03-31

### Added
- Signed install, setup, and update flow with a canonical public onboarding path.
- Native Carrier chat with signed message verification, cross-host WSL ↔ Jetson proof, and same-host native ↔ WASM proof coverage.
- Capability-gated capsule execution across native runtime surfaces, WASM capsules, and microVM capsules.
- DID-backed identity, local sharing, site hosting/publish/activate/rollback, and agent capsule support.

### Changed
- The public repository starts fresh at `0.1.0`.
- `elastos chat` is native Carrier chat only; packaged chat surfaces launch through `elastos capsule ...`.
- The installer and first-run story are centered on `install.sh -> elastos setup -> elastos`.

### Removed
- Runtime/proof override residue including `ELASTOS_COMPONENTS_MANIFEST`, `ELASTOS_DEV_SEARCH`, `SkippedDevPath`, `InstalledBinaryVerification`, and `chat --mode ...`.

## Pre-public internal lineage

Earlier internal release candidates and development history existed before the public repository launch. They are intentionally not carried forward as the public release line.
