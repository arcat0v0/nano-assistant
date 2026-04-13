---
name: container-orchestration
description: Container orchestration guide — Docker Compose, Podman rootless, networking, volumes, multi-stage builds, and best practices
version: 0.1.0
author: nano-assistant
tags: [docker, podman, compose, container, orchestration, networking, volumes]
---

# Container Orchestration

Use this skill when the user asks about Docker, Podman, container orchestration, compose files, container networking, volumes, image building, or related topics. Follow each section's instructions carefully and apply them to the user's specific context.

---

## 1. Docker Compose

### compose.yml Syntax

When writing a compose file, always use the `compose.yml` filename (preferred over `docker-compose.yml`). Structure services with explicit networks and named volumes. Never use `version:` — it is obsolete in Compose v2+.

```yaml
# compose.yml — web + db + cache stack
services:
  web:
    build:
      context: .
      target: runtime          # multi-stage target
    image: myapp:latest
    ports:
      - "8080:8080"
    environment:
      - DATABASE_URL=${DATABASE_URL}
    env_file:
      - .env
    depends_on:
      db:
        condition: service_healthy
      cache:
        condition: service_started
    networks:
      - backend
    restart: unless-stopped
    deploy:
      resources:
        limits:
          memory: 512m
          cpus: "0.5"

  db:
    image: postgres:16-alpine
    volumes:
      - pg_data:/var/lib/postgresql/data
    environment:
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
      POSTGRES_DB: ${POSTGRES_DB:-app}
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 30s
    networks:
      - backend

  cache:
    image: redis:7-alpine
    volumes:
      - redis_data:/data
    command: redis-server --save 60 1 --loglevel warning
    networks:
      - backend

networks:
  backend:
    driver: bridge

volumes:
  pg_data:
  redis_data:
```

### Environment Variables

Always use an `.env` file for secrets and environment-specific values. Never hardcode credentials.

```bash
# .env (never commit this file)
DATABASE_URL=postgres://postgres:secret@db:5432/app
POSTGRES_PASSWORD=secret
POSTGRES__DB=app
```

Add `.env` to `.gitignore`. Reference variables in compose with `${VAR}` or `${VAR:-default}`.

### Profiles

Use profiles to conditionally start services (e.g., dev tools, observability):

```yaml
services:
  adminer:
    image: adminer
    profiles: [debug]
    ports:
      - "8081:8080"
```

Start with a profile: `docker compose --profile debug up`

### Essential Commands

```bash
docker compose up -d                    # Start all services detached
docker compose up -d --build            # Rebuild images then start
docker compose down                     # Stop and remove containers
docker compose down -v                  # Also remove named volumes
docker compose logs -f web              # Follow logs for a service
docker compose exec web sh              # Shell into running container
docker compose ps                       # List service status
docker compose pull                     # Pull latest images
docker compose restart web              # Restart one service
docker compose config                   # Validate and print resolved config
```

---

## 2. Podman Rootless

### Installation

```bash
# Fedora/RHEL
sudo dnf install podman podman-compose

# Debian/Ubuntu
sudo apt install podman

# Arch
sudo pacman -S podman podman-compose
```

### Rootless Setup

After installing, configure user namespaces:

```bash
# Enable persistent user session (lingering) — required for rootless systemd
loginctl enable-linger $(whoami)

# Verify subuid/subgid entries exist (usually auto-created)
grep $(whoami) /etc/subuid    # e.g., username:100000:65536
grep $(whoami) /etc/subgid

# If missing, add them
sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 $(whoami)
podman system migrate
```

### Docker vs Podman Command Equivalents

| Docker | Podman | Notes |
|---|---|---|
| `docker run` | `podman run` | Drop-in replacement |
| `docker build` | `podman build` | Uses Buildah under the hood |
| `docker push` | `podman push` | |
| `docker images` | `podman images` | |
| `docker ps` | `podman ps` | |
| `docker exec` | `podman exec` | |
| `docker volume` | `podman volume` | |
| `docker network` | `podman network` | |
| `docker compose` | `podman compose` or `podman-compose` | Requires separate install |
| `docker login` | `podman login` | Per-registry auth |
| `docker system prune` | `podman system prune` | |

### Podman-Compose

```bash
podman-compose up -d
podman-compose down
podman-compose logs -f
```

### Pods Concept

Pods group containers sharing a network namespace (like Kubernetes pods):

```bash
podman pod create --name myapp -p 8080:8080
podman run -d --pod myapp nginx
podman run -d --pod myapp redis
podman pod ps
podman pod stop myapp
```

### Systemd Integration

Generate a systemd unit from a running container or pod:

```bash
# Generate unit file for a container
podman generate systemd --name mycontainer --files --new

# Move to user systemd directory
mv container-mycontainer.service ~/.config/systemd/user/

# Enable and start
systemctl --user daemon-reload
systemctl --user enable --now container-mycontainer

# For pod
podman generate systemd --name mypod --files --new
```

Use `--new` so systemd recreates the container on start rather than reusing a stopped one.

---

## 3. Container Networking

### Network Drivers

| Driver | Use Case |
|---|---|
| `bridge` | Default; isolated virtual network on a single host |
| `host` | Container shares host network stack; no isolation |
| `macvlan` | Container gets its own MAC/IP on the physical network |
| `overlay` | Multi-host networking (Docker Swarm / Podman quadlets) |
| `none` | No networking |

### Custom Bridge Networks

Always create explicit named networks instead of relying on the default bridge. Named networks provide automatic DNS resolution between containers.

```bash
docker network create --driver bridge mynet
docker run -d --network mynet --name api myapp
docker run -d --network mynet --name db postgres:16
# 'api' container can reach 'db' at hostname 'db'
```

### Inter-Container DNS

On custom bridge networks, containers resolve each other by **service name** (Compose) or **container name** (standalone). The default `bridge` network does NOT provide DNS — always use a named network.

```bash
# Inside a container on a custom network:
curl http://db:5432       # resolves by container/service name
curl http://cache:6379
```

### Port Publishing Patterns

```bash
# Bind to all interfaces (default)
-p 8080:8080

# Bind to localhost only (preferred for security)
-p 127.0.0.1:8080:8080

# Expose a range
-p 9000-9005:9000-9005

# UDP
-p 514:514/udp
```

Prefer binding to `127.0.0.1` for services not meant to be publicly exposed.

---

## 4. Volume Management

### Volume Types

| Type | Syntax | Use Case |
|---|---|---|
| Named volume | `mydata:/app/data` | Persistent data managed by Docker/Podman |
| Bind mount | `./local:/app/data` | Dev: sync local files into container |
| tmpfs | `type=tmpfs,target=/tmp` | Ephemeral in-memory storage |

### Named Volumes (preferred for production)

```bash
docker volume create mydata
docker run -v mydata:/app/data myimage
docker volume inspect mydata
docker volume ls
docker volume prune    # remove unused volumes
```

### Backup Strategies

```bash
# Backup a named volume to a tar archive
docker run --rm \
  -v mydata:/source:ro \
  -v $(pwd):/backup \
  alpine tar czf /backup/mydata-$(date +%Y%m%d).tar.gz -C /source .

# Restore
docker run --rm \
  -v mydata:/target \
  -v $(pwd):/backup \
  alpine tar xzf /backup/mydata-20240101.tar.gz -C /target
```

### Data-Only Container Pattern

For sharing volumes between containers without a running process:

```bash
docker create --name datastore -v /app/data alpine
docker run --volumes-from datastore myapp
docker run --volumes-from datastore mybackuptool
```

### Volume Drivers

For distributed storage, specify a driver in compose:

```yaml
volumes:
  shared_data:
    driver: local
    driver_opts:
      type: nfs
      o: "addr=192.168.1.100,rw"
      device: ":/exports/data"
```

---

## 5. Image Building Best Practices

### Multi-Stage Builds (builder pattern)

Always use multi-stage builds to keep runtime images small. Separate build dependencies from runtime.

```dockerfile
# Stage 1: build
FROM golang:1.22-alpine AS builder
WORKDIR /app
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -o /app/server ./cmd/server

# Stage 2: runtime (minimal)
FROM scratch
COPY --from=builder /app/server /server
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
EXPOSE 8080
ENTRYPOINT ["/server"]
```

For interpreted languages, separate install from runtime:

```dockerfile
FROM node:20-alpine AS deps
WORKDIR /app
COPY package*.json ./
RUN npm ci --omit=dev

FROM node:20-alpine AS runtime
WORKDIR /app
COPY --from=deps /app/node_modules ./node_modules
COPY . .
USER node
CMD ["node", "server.js"]
```

### Layer Caching Optimization

Order Dockerfile instructions from least-to-most frequently changing:

1. Base image and system packages
2. Dependency manifests (`package.json`, `go.mod`, `Cargo.toml`)
3. Install dependencies (`npm ci`, `go mod download`)
4. Copy application source
5. Build

This ensures dependency layers are cached and only invalidated when manifests change.

### .dockerignore

Always create `.dockerignore` to prevent leaking secrets and bloating the build context:

```
.git
.env
*.env.*
node_modules
dist
build
__pycache__
*.pyc
.DS_Store
*.log
```

### Base Image Selection

| Base | Size | Use Case |
|---|---|---|
| `scratch` | 0 MB | Statically compiled binaries (Go, Rust) |
| `distroless` | ~2 MB | Minimal runtime, no shell (security-focused) |
| `alpine` | ~5 MB | General purpose; has shell + apk |
| `debian-slim` | ~75 MB | Better glibc compatibility |

Prefer `alpine` or `distroless` for production. Avoid `latest` tags — always pin to a specific version.

### COPY vs ADD

Always use `COPY` unless you specifically need `ADD`'s extra features.

- `COPY` — simple, predictable, copies local files
- `ADD` — also extracts tar archives and fetches URLs; avoid for clarity

```dockerfile
# Correct
COPY . .

# Only use ADD for extracting tarballs
ADD app.tar.gz /app/
```

### Security Scanning

Run Trivy before pushing images:

```bash
# Scan a local image
trivy image myapp:latest

# Scan in CI (fail on HIGH/CRITICAL)
trivy image --exit-code 1 --severity HIGH,CRITICAL myapp:latest

# Scan a Dockerfile for misconfigurations
trivy config .
```

---

## 6. Logging & Monitoring

### Viewing Logs

```bash
docker logs mycontainer                 # All logs
docker logs -f mycontainer             # Follow (tail -f equivalent)
docker logs --tail 100 mycontainer     # Last 100 lines
docker logs --since 1h mycontainer     # Logs from the past hour
docker logs --timestamps mycontainer   # Include timestamps

# Podman equivalents (identical flags)
podman logs -f mycontainer
```

### Log Drivers

Configure log driver in compose or at runtime:

```yaml
# compose.yml
services:
  web:
    logging:
      driver: json-file           # default; logs stored on disk as JSON
      options:
        max-size: "10m"           # rotate at 10 MB
        max-file: "3"             # keep 3 rotated files

    # OR use journald (integrates with systemctl / journalctl)
    logging:
      driver: journald
      options:
        tag: "{{.Name}}"
```

Query journald logs: `journalctl CONTAINER_NAME=mycontainer -f`

### Resource Limits

Always set memory and CPU limits in production to prevent noisy-neighbour issues:

```bash
# At runtime
docker run --memory 512m --cpus 0.5 myapp

# In compose (preferred)
deploy:
  resources:
    limits:
      memory: 512m
      cpus: "0.5"
    reservations:
      memory: 128m
```

### Monitoring

```bash
docker stats                        # Live resource usage (all containers)
docker stats mycontainer            # Single container
docker stats --no-stream            # One-shot snapshot

# Podman
podman stats
podman top mycontainer              # Process list inside container

# System-wide disk usage
docker system df
docker system prune -a --volumes    # Clean everything (use carefully)
```

For persistent monitoring, integrate with Prometheus using `cadvisor` alongside your compose stack, or use `docker stats` output piped to a logging system.

---

## General Guidelines

- Always set `restart: unless-stopped` (or `always`) in production compose files.
- Never run containers as root unless required — use `USER` in Dockerfile or `--user` flag.
- Pin all image versions (`postgres:16-alpine`, not `postgres:latest`).
- Use health checks for all stateful services so `depends_on` with `condition: service_healthy` works correctly.
- Scan images regularly with Trivy in CI pipelines.
- Prefer named volumes over bind mounts in production for portability.
- Use `podman` drop-in replacement commands when the user is on a rootless or RHEL-based system.
