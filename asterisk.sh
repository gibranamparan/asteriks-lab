#!/bin/bash

COMPOSE_FILE="$(dirname "$0")/docker-compose.yml"
CONTAINER_NAME="asterisk-server"

case "$1" in
    start)
        docker compose -f "$COMPOSE_FILE" up -d
        ;;
    stop)
        docker compose -f "$COMPOSE_FILE" down
        ;;
    restart)
        docker compose -f "$COMPOSE_FILE" down
        docker compose -f "$COMPOSE_FILE" up -d
        ;;
    logs)
        docker compose -f "$COMPOSE_FILE" logs -f
        ;;
    status)
        docker compose -f "$COMPOSE_FILE" ps
        ;;
    cli)
        docker exec -it "$CONTAINER_NAME" asterisk -rvvv
        ;;
    shell)
        docker exec -it "$CONTAINER_NAME" /bin/sh
        ;;
    reload)
        case "$2" in
            dialplan)
                docker exec -it "$CONTAINER_NAME" asterisk -rx "dialplan reload"
                ;;
            pjsip)
                docker exec -it "$CONTAINER_NAME" asterisk -rx "pjsip reload"
                ;;
            ari)
                docker exec -it "$CONTAINER_NAME" asterisk -rx "ari reload"
                ;;
            all)
                docker exec -it "$CONTAINER_NAME" asterisk -rx "core reload"
                ;;
            *)
                echo "Usage: $0 reload {dialplan|pjsip|ari|all}"
                exit 1
                ;;
        esac
        ;;
    ari-test)
        curl -s -u asterisk:asterisk http://localhost:8088/ari/asterisk/info | head -20
        ;;
    *)
        echo "Asterisk Control Script"
        echo ""
        echo "Usage: $0 {command}"
        echo ""
        echo "Commands:"
        echo "  start       Start the Asterisk container"
        echo "  stop        Stop the Asterisk container"
        echo "  restart     Restart the Asterisk container"
        echo "  logs        Follow container logs"
        echo "  status      Show container status"
        echo "  cli         Open Asterisk CLI"
        echo "  shell       Open shell in container"
        echo "  reload      Reload config: dialplan|pjsip|ari|all"
        echo "  ari-test    Test ARI API connection"
        exit 1
        ;;
esac
