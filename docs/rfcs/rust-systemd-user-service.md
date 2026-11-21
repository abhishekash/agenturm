# Linux systemd user service

Agenturm can install the foreground Rust collector as a Linux systemd user
service. Build and install the Rust binaries first, then install the service:

```sh
agenturm service install \
  --config "$HOME/.config/agenturm/sources.json" \
  --db "$HOME/.local/share/agenturm/agenturm.sqlite"
agenturm service status
journalctl --user -u agenturm-collector.service -f
```

The installer validates the collection config, writes a private unit file at
`$XDG_CONFIG_HOME/systemd/user/agenturm-collector.service` (or
`$HOME/.config/systemd/user/agenturm-collector.service`), reloads the user
manager, enables the service for `default.target`, and starts it. The unit uses
the current Agenturm executable, absolute config and database paths, and the
same foreground `collect --watch` lifecycle used by manual runs. Re-running
`service install` updates the unit and restarts the collector.
`agenturm service status` prints active, inactive, or failed state without
treating an inactive unit as a command error.

```sh
agenturm service uninstall
```

Uninstall stops and disables the unit before removing it. The collector writes
cycle reports to the systemd journal. Cycle failures are written to the journal
while the watch process remains active; failed adapter startup is retried once
per second. Systemd's bounded
restart policy handles collector process exits, not source-level failures. The
generated unit uses a private umask, disallows privilege elevation, and grants
write access to the database directory. Source roots cannot overlap that
directory. Adapter child processes share this permission and run as the same
user, so this service does not isolate Store authority from an untrusted
adapter; use only trusted adapters until a separate storage identity or tighter
child sandbox is available.

This backend manages the current user's systemd manager on Linux. It does not
install a system-wide service or provide launchd/Windows service support. By
default, a user manager starts with the user's login session; use systemd's
linger setting only if collection must continue without an active login. If
installation writes the unit but `systemctl --user` is unavailable, the error
includes the unit path so it can be inspected or removed manually.
