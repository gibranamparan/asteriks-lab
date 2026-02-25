# asteriks-lab

This project runs Asterisk plus a Rust sidecar agent that syncs `intercom` data from HEADEND events into [`config/pjsip.conf`](config/pjsip.conf).

## Services

- [`asterisk`](docker-compose.yml:2): PBX container (`andrius/asterisk:latest`)
- [`intercom-sync-agent`](docker-compose.yml:13): Rust agent that:
  - subscribes to AMQP queue bindings
  - filters `intercom` create/update/delete events
  - fetches full intercom list from GraphQL
  - regenerates `pjsip.conf`
  - safely applies/rolls back config
  - reloads PJSIP (and restarts Asterisk if reload fails)

## Required environment variables

Loaded from [`.env`](.env) via [`env_file`](docker-compose.yml:23) and passed through in [`docker-compose.yml`](docker-compose.yml:25):

- `HEADEND_URL` (host/IP only, no scheme)
- `HEADEND_AMQP_EXCHANGE`
- `HEADEND_AMQP_EXCHANGE_TYPE`
- `HEADEND_AMQP_QUEUE`
- `HEADEND_AMQP_ROUTING_KEY`
- `HEADEND_AMQP_USERNAME`
- `HEADEND_AMQP_PASSWORD`
- `PJSIP_BASE_DIR`
- `PJSIP_BACKUP_DIR`

No defaults are provided in compose interpolation. Missing or empty values will cause startup/config failure, and the agent logs the exact reason in [`required_env()`](agent/src/main.rs:527) and startup config validation in [`main()`](agent/src/main.rs:112).

Derived endpoints inside the agent (from `HEADEND_URL`):

- AMQP: `amqp://<HEADEND_URL>:5672`
- GraphQL: `http://<HEADEND_URL>:5000/graphql`

## Agent behavior

Implemented in [`agent/src/main.rs`](agent/src/main.rs:1).

- Event schema expected from queue message body:
  - `id: string`
  - `event: string`
  - `resource: string`
- Relevant events: `resource == "intercom"` and `event in {create, update, delete}`
- Debounce window: internal constant `10s`
- Periodic full reconciliation: internal constant `3600s`
- Startup sync: runs once at boot

## pjsip generation and safe apply

Paths (configured in [`.env`](.env:8)):

- active: `PJSIP_BASE_DIR/pjsip.conf`
- last good: `PJSIP_BASE_DIR/pjsip.conf.last-good`
- rolling backups: `PJSIP_BACKUP_DIR`

Generation details:

- username is normalized MAC (lowercase, no `:`)
- shared password is currently hardcoded as `Sentrics2026`
- output is deterministic (sorted by normalized MAC)
- unchanged file content is skipped (hash compare)

Apply/rollback flow:

1. backup current config
2. write in-place (truncate + write + fsync) to preserve inode for single-file bind mounts
3. run Asterisk reload (`pjsip reload`, fallback to `module reload res_pjsip.so`)
4. on failure: restore `last-good` and run `docker restart asterisk-server`

## Mount strategy (important)

In [`docker-compose.yml`](docker-compose.yml:1):

- Asterisk uses **individual file mounts** into `/etc/asterisk`:
  - [`config/asterisk.conf`](config/asterisk.conf)
  - [`config/modules.conf`](config/modules.conf)
  - [`config/ari.conf`](config/ari.conf)
  - [`config/http.conf`](config/http.conf)
  - [`config/extensions.conf`](config/extensions.conf)
  - [`config/pjsip.conf`](config/pjsip.conf)
- Agent mounts:
  - [`./config:/config`](docker-compose.yml:35)
  - [`./backups:/backups`](docker-compose.yml:36)

This avoids shadowing the entire `/etc/asterisk` directory (which breaks boot if core files are missing), while still allowing the agent to update the same [`pjsip.conf`](config/pjsip.conf) file used by Asterisk.

## Run

Start stack:

```bash
./asterisk.sh start
```

Stop stack:

```bash
./asterisk.sh stop
```

Follow logs:

```bash
./asterisk.sh logs
```

## Notes / good practices already included

- Debounce to avoid excessive reload/restart churn
- Full periodic reconciliation to correct drift
- Last-good rollback protection
- Backup retention cleanup (keeps newest 20 backups)
- Startup reconciliation so restarts converge quickly
