# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is an Asterisk PBX lab environment running in Docker. Asterisk is an open-source telephony/VoIP framework.

## Commands

Use `./asterisk.sh` to control the container:

```bash
./asterisk.sh start          # Start container
./asterisk.sh stop           # Stop container
./asterisk.sh restart        # Restart container
./asterisk.sh logs           # Follow logs
./asterisk.sh status         # Show status
./asterisk.sh cli            # Open Asterisk CLI
./asterisk.sh shell          # Open shell in container
./asterisk.sh reload dialplan|pjsip|ari|all   # Reload configs
./asterisk.sh ari-test       # Test ARI connection
```

## Architecture

- **docker-compose.yml**: Runs `andrius/asterisk:latest` with host networking (ports 5060/UDP for SIP, 8088/TCP for ARI)
- **config/pjsip.conf**: PJSIP configuration defining SIP transport, endpoint/auth/AOR templates, and extensions (100, 101)
- **config/extensions.conf**: Dialplan with `internal` context for routing calls between extensions and test services
- **config/http.conf**: HTTP server configuration for ARI (binds to port 8088)
- **config/ari.conf**: ARI REST API configuration with user credentials

### SIP Endpoints

Extensions 100 and 101 are configured with username/password auth. Templates (`endpoint-template`, `auth-template`, `aor-template`) provide reusable configuration.

### Dialplan

- `_1XX`: Dial pattern for extensions 100-199 via PJSIP
- `600`: Echo test service
- `601`: Audio playback test

### ARI (Asterisk REST Interface)

- **Endpoint**: `http://localhost:8088/ari/`
- **Credentials**: username `asterisk`, password `asterisk`
- **WebSocket**: `ws://localhost:8088/ari/events?app=<app_name>&api_key=asterisk:asterisk`
