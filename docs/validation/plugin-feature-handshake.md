# Process feature handshake checkpoint

19 November 2026. An approved manifest feature is now checked against the running child's handshake before the first poll. A configured source root requires the child to advertise `configured-root/1`; an explicit built-in source replacement requires its `harness.<name>/1` feature. The host also rejects any child-advertised feature absent from the manifest. A disagreement closes the child and leaves its source failure visible to the supervisor. The trusted reader does not start alongside a failed replacement.

The manifest and config remain inertly validated before Store creation. The child feature check necessarily occurs when the process starts; it does not prove data parity or source truth. The handshake list is bounded, unique and syntactically validated.

Validation: 34 focused process, config, supervisor, runtime and standalone sample tests passed. Injected fixtures cover a manifest that claims a harness or configured root the child omits, and a child that advertises an unapproved feature. The installed sample still reaches receiver health. `compileall`, HTML parsing and `git diff --check` passed.
