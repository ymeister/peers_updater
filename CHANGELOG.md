# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/).
<!-- and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).-->

<!-- Use this as a template
## [X.Y.Z] - YYYY-MM-DD
### Added
- for new features.

### Changed
- for changes in existing functionality.

### Deprecated
- for soon-to-be removed features.

### Removed
- for now removed features.

### Fixed
- for any bug fixes.

### Security
- in case of vulnerabilities.
-->
## [Unreleased]

### Added

- Added the `-s` (`--socket`) option — the address of the Yggdrasil admin API socket.

### Changed

- API mode (`-a`) no longer reads the Yggdrasil configuration file. The admin socket address is taken from the `-s` option or its default (`unix:///run/yggdrasil/yggdrasil.sock`; on Windows `tcp://localhost:9001`), so `-a` works without a configuration file. The configuration file (`-c`) is now used only together with `-u`; in builds without the `updating_cfg` feature the `-c` option no longer exists.
- The `-r` (`--restart`) option now takes effect only together with `-u`.
- A failure to communicate with the admin API in `-a` mode now results in a non-zero exit code.
- In API mode, if exactly `-n` healthy managed peers (outbound, public, not in `-e`) are already connected and there are no down or duplicate managed connections to clean up, the utility now exits before downloading the list of public peers, making scheduled runs cheap. Otherwise the managed peers are reconciled with the fresh top-`-n` set: the ones still among the best are kept instead of being removed and re-added, the excess ones are removed. With the `-i`/`-I` filters and with `-u` the early exit is skipped and a full reconciliation is done, so already-connected peers matching the filters get replaced and the daemon stays in sync with the updated configuration file.
- Peers are identified by their host everywhere (counting, extras matching, reconciliation), the port is disregarded: at most one managed connection per host is kept, an extra (`-e`) peer protects all connections to its host, and duplicate connections to one host are trimmed.
- Extra (`-e`) peers are added via the API only when they are not already configured (a configured but currently down extra is not re-added). Extra URIs that cannot be parsed are matched literally and reported with a warning.
- On daemons that do not report the peer direction in `getpeers` (Yggdrasil <= 0.4.7), peers are neither counted nor removed; the utility degrades to only adding peers.
- If the `getpeers` reply cannot be parsed, the utility no longer aborts: it adds up to `-n` peers and the extras without removing anything.
- Failed `addpeer`/`removepeer` API requests are now reported instead of being silently ignored, and requests that cannot be delivered to the daemon make the exit code non-zero.

### Fixed

- Fixed a runtime panic in builds made with `--no-default-features --features using_api` (accessing the unregistered `-r` argument).
- `-n 0` no longer adds all downloaded peers (both in the configuration file and via the API).
- Peers that did not respond to ping are no longer added.
- The default admin socket address on Windows is now a valid URI (`tcp://localhost:9001`).
- The admin socket URI parser now accepts uppercase letters in hostnames and the correct `quic://` scheme spelling.
- The write-access pre-check of the configuration file now checks actual access rights instead of the read-only attribute.
- API mode no longer removes peers it does not manage: multicast (link-local) peers, peers on loopback, private (RFC1918) and unique-local addresses, inbound connections, extra (`-e`) peers and peers with unrecognized URIs (e.g. `socks://`) are kept.
- Non-public peers are now detected by parsing the address instead of URI substring matching, so public peers whose URI merely contains `fe80:`, `169.254.` or `%` are managed normally.
- `-a -n 0` removes the managed peers again (keeping the extras) without downloading the peers list, and lowering `-n` trims the excess.
- If some or all downloaded peers do not respond to ping, API mode keeps the already connected healthy peers in the unfilled slots instead of removing them without replacement.
- `-p` no longer fails on an invalid `-n` value (all other parameters are ignored with `-p`, as documented).

## [0.3.4] - 2024-08-06

### Added

- Added WebSocket-peers support (ws:// | wss://)

## [0.3.3] - 2023-11-07

### Changed

- Сhanged default paths: `/etc/yggdrasil.conf` -> `/etc/yggdrasil/yggdrasil.conf`; `/var/run/yggdrasil.sock` -> `/var/run/yggdrasil/yggdrasil.sock`

## [0.3.2] - 2023-10-28

### Added

- Adaptation for Yggdrasil v. 0.5 (quic:// support)

## [0.3.1] - 2023-03-01

### Fixed

- Error while loading shared libraries (libssl.so) (#10).

## [0.3.0] - 2023-03-01

### Added

- Added the ability to simply disable some functionality during assembly. For more information, see [README.md](README.md#build-from-source) 

### Changed

- The number of dependencies has been reduced and some of them have been replaced with "lighter" alternatives. As a result, smaller binaries are obtained.
- Added a couple of notes about the [build](README.md#build-from-source) from source

## [0.2.0] - 2023-02-23

### Added

- Added the `-S` (`--self_update`) option. 

Starting from this version, it will be possible to update the utility by running it with the `-S` option. This is done for the simplicity and convenience of updating. You will no longer need to manually download the release from GitHub, unpack and manually replace the executable file - the utility will do all this automatically.

**Usage example:**

Checking the version:
```
./peers_updater -V
Yggdrasil peers updater 0.2.0
```
```
./peers_updater -S
New release found: 0.2.0 --> 0.2.1
Downloading...
Extracting...
Replacing binary file...
Done.
```
Checking the version again:
```
./peers_updater -V
Yggdrasil peers updater 0.2.1
```
Here we see that the program has been successfully updated.

The utility with the `-S` option can be run on a schedule (cron, windows scheduler) or manually.

## [0.1.0] - 2023-02-18

### Added

- Added the `-I` (`--ignore_country`) option. Related to #6

### Changed

- Updated versions of dependencies (clap, nu-json, regex, attohttpc)

Some antiviruses may falsely trigger on compressed UPX binary files, so I added uncompressed files as well.

## [0.0.9] - 2023-01-26

### Fixed

- adding one peer twice with different protocols (#5)

Some antiviruses may falsely trigger on compressed UPX binary files, so I added uncompressed files as well.

## [0.0.8] - 2023-01-07

### Changed

- optimizing the size of executable files.

## [0.0.7] - 2022-12-17

### Fixed

- fix incorrect behavior without the `-i` option.

## [0.0.6] - 2022-12-15

### Changed

- If the peer URI contains at least one of the strings that are passed with the `-i` option, it will be ignored (see the example in [README.md](README.md)).

## [0.0.5] - 2022-12-14

### Fixed

- Fix pinging ipv6 nodes (by @parnikkapore)
- Checking conf access only if necessary

## [0.0.4] - 2022-12-13

### Changed

- Comments will no longer be deleted from the configuration file.

## [0.0.3] - 2022-12-10

### Changed

- The work with the admin api has been redesigned, the messages about problems will be more informative.
- Added the `-u` (`--update_cfg`) option. Changes to the configuration file will be made only if this parameter is specified.

## [0.0.2] - 2022-12-02

### Added

- parameter `-i` (`--ignore`) , where value is a space-separated string, in which you can specify the URI of peers that should be ignored

### Changed

- Additional peers (extra) will be added not only in the config, but also using the API
- Minor improvements, optimization

## [0.0.1] - 2022-11-29

### Added

- First commit.
- Initial public release.
