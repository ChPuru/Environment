# Deployment guide

These are CLI tools, not services (except relay, which is a server, and
ferry recv / warden-over-S3, which are long-running or remote). Here is
how to run the ones people actually schedule or expose — and the security
truths you must not skip.

## warden: scheduled, verified backups

A real backup policy is three cron lines: back up, prove, and reclaim.

    # /etc/cron.d/warden  (adjust paths, user, repo)
    # env for all three:
    WARDEN_PASSPHRASE=...        # see "secrets" below — do NOT inline in prod
    30 2 * * *  you  warden --repo /srv/backups backup /home/you >> /var/log/warden.log 2>&1
    0  3 * * 0  you  warden --repo /srv/backups verify --deep   >> /var/log/warden.log 2>&1
    0  4 1 * *  you  warden --repo /srv/backups prune           >> /var/log/warden.log 2>&1

Then monitor freshness — `health` exits nonzero when the repo is stale,
unproven, or shows a problem, which is exactly what a monitoring check
wants:

    warden --repo /srv/backups health --max-verify-age 8 --max-drill-age 35 \
      || alert "backups unproven"

### warden to S3 / MinIO

    export WARDEN_S3_ACCESS_KEY=... WARDEN_S3_SECRET_KEY=...
    export WARDEN_S3_ENDPOINT=https://s3.us-east-1.amazonaws.com   # or MinIO URL
    warden --repo s3://your-bucket/backups init
    warden --repo s3://your-bucket/backups backup /home/you

The bucket must already exist and must support conditional writes
(If-None-Match) — AWS S3 and MinIO both do; that is what makes init and
the writer lock atomic. If a backup process is killed, clear its lock with
`warden --repo ... unlock` (keyless) or wait out the TTL.

### Rollback detection (the freshness pin)

`verify` records, per client, the newest snapshot it has seen, in a file
OUTSIDE the repo (XDG cache; override with WARDEN_PIN_DIR). If the repo
later looks older (restored from an old copy, tampered rollback), verify
fails loudly. Multi-client caveat: a `forget` on another client can
false-alarm this one until it backs up again — documented, not a bug.

## relay: running the chat server

    relay serve 4000     # foreground; wrap in systemd for real use

    # /etc/systemd/system/relay.service
    [Unit]
    Description=relay chat
    After=network.target
    [Service]
    ExecStart=/usr/local/bin/relay serve 4000
    Restart=on-failure
    User=relay
    [Install]
    WantedBy=multi-user.target

Bind it behind something that gives you TLS if it faces a network: relay
speaks plaintext and has no authentication by design. Do not expose it
raw to the internet.

## ferry: receiving files

    ferry recv --port 3777 --dir /srv/incoming    # one transfer per connection, forever

Same warning: plaintext over TCP. Fine on a trusted LAN or over an SSH
tunnel / WireGuard; do not expose the port publicly for sensitive data.

## Security truths (read this)

- warden encrypts and authenticates repository DATA (XChaCha20-Poly1305,
  Argon2 KDF). Your passphrase is the whole game — lose it, lose the
  backup; leak it, leak the backup.
- Secrets in cron: do not inline WARDEN_PASSPHRASE / WARDEN_S3_SECRET_KEY
  in world-readable crontabs. Use a root-only EnvironmentFile with systemd
  timers, a secrets manager, or an interactive wrapper. This repository does
  not ship a secrets solution.
- ferry and relay are UNENCRYPTED transports. Tunnel them.
- These tools were built and tested as a portfolio, not hardened for
  hostile production exposure. Treat them accordingly: trusted inputs,
  trusted networks, real backups of anything that matters.
