---
name: database-admin
description: Database administration guide covering PostgreSQL, MySQL/MariaDB, Redis, and SQLite — installation, user management, backup/restore, performance tuning, and troubleshooting
version: 0.1.0
author: nano-assistant
tags: [database, postgresql, mysql, mariadb, redis, sqlite, admin, backup, performance]
---

# Database Administration Skill

This skill provides guidance for administering PostgreSQL, MySQL/MariaDB, Redis, and SQLite databases. When a user asks about database setup, user management, backups, performance tuning, replication, or troubleshooting, follow the relevant section below.

Always confirm the OS (Debian/Ubuntu, RHEL/Fedora, Arch) and database version before suggesting install commands. Prefer showing the actual commands the user should run rather than abstract descriptions.

---

## 1. PostgreSQL

**When to use this section**: user is working with PostgreSQL (psql, pg_dump, postgresql.conf, pg_hba.conf, replication, logical decoding, etc.).

### Installation

**Debian / Ubuntu**
```bash
sudo apt update
sudo apt install -y postgresql postgresql-contrib
sudo systemctl enable --now postgresql
```

**RHEL / Fedora / Rocky**
```bash
# Install from the official PGDG repo for a specific version (e.g., 16)
sudo dnf install -y https://download.postgresql.org/pub/repos/yum/reporpms/EL-9-x86_64/pgdg-redhat-repo-latest.noarch.rpm
sudo dnf -qy module disable postgresql
sudo dnf install -y postgresql16-server
sudo /usr/pgsql-16/bin/postgresql-16-setup initdb
sudo systemctl enable --now postgresql-16
```

**Arch Linux**
```bash
sudo pacman -S postgresql
sudo -u postgres initdb --locale=en_US.UTF-8 -D /var/lib/postgres/data
sudo systemctl enable --now postgresql
```

### User and Database Management

```bash
# Switch to the postgres system user
sudo -u postgres psql

-- Create a superuser
CREATE USER admin_user WITH SUPERUSER PASSWORD 'strongpassword';

-- Create a regular application user
CREATE USER app_user WITH PASSWORD 'apppassword';

-- Create a database owned by the app user
CREATE DATABASE myapp OWNER app_user;

-- Grant privileges on an existing database
GRANT ALL PRIVILEGES ON DATABASE myapp TO app_user;

-- Grant schema privileges (PostgreSQL 15+ requires explicit schema grants)
\c myapp
GRANT USAGE ON SCHEMA public TO app_user;
GRANT CREATE ON SCHEMA public TO app_user;

-- List users
\du

-- List databases
\l

-- Change a user's password
ALTER USER app_user WITH PASSWORD 'newpassword';

-- Drop a user (revoke ownership first)
REASSIGN OWNED BY app_user TO postgres;
DROP OWNED BY app_user;
DROP USER app_user;
```

### Backup and Restore

```bash
# Logical dump of a single database (plain SQL)
pg_dump -U postgres -h localhost myapp > myapp_$(date +%Y%m%d).sql

# Compressed custom format (recommended for large databases)
pg_dump -U postgres -Fc myapp > myapp_$(date +%Y%m%d).dump

# Dump all databases + global objects (roles, tablespaces)
pg_dumpall -U postgres > all_databases_$(date +%Y%m%d).sql

# Restore from plain SQL dump
psql -U postgres -d myapp < myapp_20240101.sql

# Restore from custom format dump
pg_restore -U postgres -d myapp --no-owner --role=app_user myapp_20240101.dump

# Parallel restore (faster on multi-core systems)
pg_restore -U postgres -d myapp -j 4 myapp_20240101.dump
```

**Point-in-time recovery (PITR)** requires WAL archiving — see postgresql.conf tuning below.

### postgresql.conf Tuning

Key parameters to tune in `/etc/postgresql/<version>/main/postgresql.conf` (or `/var/lib/pgsql/<version>/data/postgresql.conf`):

```ini
# Memory — set shared_buffers to ~25% of RAM
shared_buffers = 4GB

# Per-query sort/hash memory — raise carefully; can multiply by max_connections
work_mem = 64MB

# OS cache estimate — affects query planner
effective_cache_size = 12GB

# Background writer checkpoint tuning (reduces I/O spikes)
checkpoint_completion_target = 0.9
wal_buffers = 64MB
max_wal_size = 4GB
min_wal_size = 1GB

# Connection settings
max_connections = 200
# For high-connection workloads, use PgBouncer instead of raising this further

# Logging slow queries
log_min_duration_statement = 500   # log queries taking > 500ms
log_line_prefix = '%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h '

# WAL archiving (required for PITR)
wal_level = replica
archive_mode = on
archive_command = 'cp %p /mnt/wal_archive/%f'
```

After editing, reload without restart when possible:
```bash
sudo -u postgres psql -c "SELECT pg_reload_conf();"
# or
sudo systemctl reload postgresql
```

### Monitoring and Diagnostics

```sql
-- Active connections and states
SELECT pid, usename, application_name, client_addr, state, wait_event_type, wait_event, query
FROM pg_stat_activity
WHERE state != 'idle'
ORDER BY query_start;

-- Count connections by state
SELECT state, count(*) FROM pg_stat_activity GROUP BY state;

-- Kill a specific backend
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid = 12345;

-- Kill all idle connections older than 10 minutes
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE state = 'idle' AND state_change < NOW() - INTERVAL '10 minutes';

-- Table sizes
SELECT schemaname, tablename,
       pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) AS total_size,
       pg_size_pretty(pg_relation_size(schemaname||'.'||tablename)) AS table_size,
       pg_size_pretty(pg_indexes_size(schemaname||'.'||tablename)) AS index_size
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog','information_schema')
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC
LIMIT 20;

-- Index usage (find unused indexes)
SELECT schemaname, tablename, indexname, idx_scan, idx_tup_read, idx_tup_fetch
FROM pg_stat_user_indexes
ORDER BY idx_scan ASC;

-- Cache hit ratio (should be > 99%)
SELECT sum(heap_blks_read) AS heap_read, sum(heap_blks_hit) AS heap_hit,
       round(sum(heap_blks_hit) * 100.0 / nullif(sum(heap_blks_hit) + sum(heap_blks_read), 0), 2) AS ratio
FROM pg_statio_user_tables;

-- Long-running transactions (dangerous for replication lag and bloat)
SELECT pid, now() - xact_start AS duration, query, state
FROM pg_stat_activity
WHERE xact_start IS NOT NULL
ORDER BY duration DESC;
```

### Replication Basics

**Primary server** — postgresql.conf:
```ini
wal_level = replica
max_wal_senders = 10
wal_keep_size = 1GB   # keep WAL segments for standbys
hot_standby = on
```

**pg_hba.conf** — allow replication user:
```
host  replication  replicator  192.168.1.0/24  scram-sha-256
```

```sql
-- Create replication user
CREATE USER replicator WITH REPLICATION ENCRYPTED PASSWORD 'replpassword';
```

**Standby server** — initial base backup:
```bash
pg_basebackup -h primary_host -U replicator -D /var/lib/postgresql/16/main \
  -P -Xs -R --checkpoint=fast
```

The `-R` flag writes `standby.signal` and connection info into `postgresql.auto.conf` automatically. Then start the standby:
```bash
sudo systemctl start postgresql
```

**Check replication lag**:
```sql
-- On primary
SELECT client_addr, state, sent_lsn, write_lsn, flush_lsn, replay_lsn,
       (sent_lsn - replay_lsn) AS replication_lag_bytes
FROM pg_stat_replication;
```

---

## 2. MySQL / MariaDB

**When to use this section**: user is working with MySQL 5.7/8.x or MariaDB 10.x/11.x (mysqldump, my.cnf, InnoDB, binary logging, SHOW PROCESSLIST, etc.).

### Installation

**Debian / Ubuntu (MySQL)**
```bash
sudo apt update
sudo apt install -y mysql-server
sudo systemctl enable --now mysql
sudo mysql_secure_installation
```

**Debian / Ubuntu (MariaDB)**
```bash
sudo apt install -y mariadb-server
sudo systemctl enable --now mariadb
sudo mysql_secure_installation
```

**RHEL / Fedora (MariaDB)**
```bash
sudo dnf install -y mariadb-server
sudo systemctl enable --now mariadb
```

**Arch Linux**
```bash
sudo pacman -S mariadb
sudo mariadb-install-db --user=mysql --basedir=/usr --datadir=/var/lib/mysql
sudo systemctl enable --now mariadb
```

### User and Privilege Management

```sql
-- Connect as root
sudo mysql -u root

-- Create a user (MySQL 8+ uses caching_sha2_password by default)
CREATE USER 'app_user'@'localhost' IDENTIFIED BY 'strongpassword';

-- For remote access
CREATE USER 'app_user'@'192.168.1.%' IDENTIFIED BY 'strongpassword';

-- Grant all privileges on a specific database
GRANT ALL PRIVILEGES ON myapp.* TO 'app_user'@'localhost';

-- Read-only replica user
GRANT SELECT, REPLICATION SLAVE, REPLICATION CLIENT ON *.* TO 'readonly'@'%' IDENTIFIED BY 'ropassword';

-- Show grants for a user
SHOW GRANTS FOR 'app_user'@'localhost';

-- Revoke a privilege
REVOKE DELETE ON myapp.* FROM 'app_user'@'localhost';

-- Apply privilege changes
FLUSH PRIVILEGES;

-- Drop a user
DROP USER 'app_user'@'localhost';

-- Create a database with charset
CREATE DATABASE myapp CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
```

### Backup and Restore

```bash
# Single database dump
mysqldump -u root -p myapp > myapp_$(date +%Y%m%d).sql

# Include stored routines and events
mysqldump -u root -p --routines --events --triggers myapp > myapp_full.sql

# All databases
mysqldump -u root -p --all-databases > all_$(date +%Y%m%d).sql

# Parallel dump with mysqlpump (faster for large databases)
mysqlpump -u root -p --default-parallelism=4 myapp > myapp_pump.sql

# Restore
mysql -u root -p myapp < myapp_20240101.sql

# Physical backup with Percona XtraBackup (for InnoDB, minimal downtime)
xtrabackup --backup --target-dir=/backup/mysql_$(date +%Y%m%d)
xtrabackup --prepare --target-dir=/backup/mysql_20240101
xtrabackup --copy-back --target-dir=/backup/mysql_20240101
```

### my.cnf Tuning

Location: `/etc/mysql/my.cnf`, `/etc/my.cnf`, or `/etc/mysql/conf.d/custom.cnf`.

```ini
[mysqld]
# InnoDB buffer pool — set to 70-80% of available RAM
innodb_buffer_pool_size = 8G

# If buffer pool > 8GB, use multiple instances
innodb_buffer_pool_instances = 8

# InnoDB log file size (larger = fewer writes, slower crash recovery)
innodb_log_file_size = 1G
innodb_log_buffer_size = 64M

# Flush method (O_DIRECT avoids double-buffering with OS cache)
innodb_flush_method = O_DIRECT

# Connections
max_connections = 300
thread_cache_size = 50

# Slow query log
slow_query_log = 1
slow_query_log_file = /var/log/mysql/slow.log
long_query_time = 1
log_queries_not_using_indexes = 1

# Binary logging (required for replication and PITR)
log_bin = /var/log/mysql/mysql-bin
binlog_format = ROW
expire_logs_days = 7
server_id = 1

# Character set
character-set-server = utf8mb4
collation-server = utf8mb4_unicode_ci
```

Restart MySQL after changing these:
```bash
sudo systemctl restart mysql
```

### Monitoring and Diagnostics

```sql
-- Show current running queries
SHOW FULL PROCESSLIST;

-- Kill a query by process ID
KILL QUERY 1234;
KILL CONNECTION 1234;  -- kill the whole connection

-- InnoDB status (lock waits, transactions, buffer pool)
SHOW ENGINE INNODB STATUS\G

-- Show variables and their current values
SHOW VARIABLES LIKE 'innodb_buffer_pool%';
SHOW STATUS LIKE 'Threads_connected';

-- Table sizes
SELECT table_schema, table_name,
       ROUND(data_length/1024/1024, 2) AS data_mb,
       ROUND(index_length/1024/1024, 2) AS index_mb,
       ROUND((data_length + index_length)/1024/1024, 2) AS total_mb
FROM information_schema.TABLES
WHERE table_schema NOT IN ('information_schema','performance_schema','mysql','sys')
ORDER BY (data_length + index_length) DESC
LIMIT 20;

-- Analyze slow query log (use pt-query-digest for deep analysis)
pt-query-digest /var/log/mysql/slow.log | head -100

-- Check replication status (on replica)
SHOW SLAVE STATUS\G   -- MySQL 5.7
SHOW REPLICA STATUS\G -- MySQL 8.0+
```

---

## 3. Redis

**When to use this section**: user is working with Redis (redis-cli, persistence, caching, pub/sub, Lua scripting, Sentinel, Cluster).

### Installation

**Debian / Ubuntu**
```bash
sudo apt install -y redis-server
sudo systemctl enable --now redis-server
```

**RHEL / Fedora**
```bash
sudo dnf install -y redis
sudo systemctl enable --now redis
```

**Arch Linux**
```bash
sudo pacman -S redis
sudo systemctl enable --now redis
```

**Verify**:
```bash
redis-cli ping   # should return PONG
redis-cli info server | grep redis_version
```

### redis-cli Basics

```bash
# Connect to local instance
redis-cli

# Connect to remote with auth
redis-cli -h 192.168.1.10 -p 6379 -a password

# Run a single command non-interactively
redis-cli SET mykey "hello"
redis-cli GET mykey

# Monitor all commands in real time (use with care in production)
redis-cli MONITOR

# Check memory usage
redis-cli INFO memory

# List all keys matching a pattern (KEYS is O(n), use SCAN in production)
redis-cli SCAN 0 MATCH "user:*" COUNT 100

# Delete keys matching a pattern (pipeline with xargs)
redis-cli --scan --pattern "session:*" | xargs redis-cli DEL
```

### Common Commands

```bash
# Strings
SET key value EX 3600    # set with 60-minute TTL
GET key
INCR counter
MSET k1 v1 k2 v2

# Hashes (good for objects)
HSET user:1 name "Alice" email "alice@example.com"
HGET user:1 name
HGETALL user:1
HINCRBY user:1 score 10

# Lists
RPUSH queue job1 job2
LPOP queue
LLEN queue
BRPOP queue 30          # blocking pop with 30s timeout

# Sets
SADD tags "redis" "database"
SMEMBERS tags
SISMEMBER tags "redis"
SINTER set1 set2

# Sorted Sets (great for leaderboards)
ZADD leaderboard 100 "alice" 200 "bob"
ZRANGE leaderboard 0 -1 WITHSCORES
ZRANK leaderboard "alice"
ZRANGEBYSCORE leaderboard 50 150

# Key expiry
EXPIRE key 3600
TTL key
PERSIST key          # remove TTL
```

### Persistence: RDB vs AOF

**RDB** (snapshotting) — `/etc/redis/redis.conf`:
```ini
# Save a snapshot if at least N keys changed in M seconds
save 900 1      # 1 key in 15 min
save 300 10     # 10 keys in 5 min
save 60 10000   # 10000 keys in 1 min

dbfilename dump.rdb
dir /var/lib/redis
```

**AOF** (append-only file) — logs every write:
```ini
appendonly yes
appendfilename "appendonly.aof"
appendfsync everysec   # options: always | everysec | no
auto-aof-rewrite-percentage 100
auto-aof-rewrite-min-size 64mb
```

**Recommendation**: Use both for maximum durability — AOF for point-in-time recovery, RDB for fast restarts.

Manual snapshot:
```bash
redis-cli BGSAVE     # async snapshot
redis-cli BGREWRITEAOF  # rewrite AOF to compact it
```

### Memory Management

```ini
# redis.conf
maxmemory 4gb

# Eviction policy when maxmemory is reached
# allkeys-lru  — evict least-recently-used keys (good for caching)
# volatile-lru — evict LRU keys with TTL set
# allkeys-lfu  — evict least-frequently-used (Redis 4+)
# noeviction   — return error (default, good for persistent data)
maxmemory-policy allkeys-lru

# Lazy freeing (async deletes, avoids blocking)
lazyfree-lazy-eviction yes
lazyfree-lazy-expire yes
```

Check memory:
```bash
redis-cli INFO memory | grep -E 'used_memory_human|maxmemory_human|mem_fragmentation_ratio'
```

High `mem_fragmentation_ratio` (> 1.5) indicates fragmentation — trigger defragmentation:
```bash
redis-cli CONFIG SET activedefrag yes
```

### Sentinel (High Availability)

Sentinel monitors a primary and promotes a replica on failure. Minimum 3 Sentinel nodes for quorum.

`/etc/redis/sentinel.conf`:
```ini
port 26379
sentinel monitor mymaster 192.168.1.10 6379 2   # quorum = 2
sentinel auth-pass mymaster secretpassword
sentinel down-after-milliseconds mymaster 5000
sentinel failover-timeout mymaster 60000
sentinel parallel-syncs mymaster 1
```

```bash
sudo systemctl enable --now redis-sentinel

# Check sentinel state
redis-cli -p 26379 SENTINEL masters
redis-cli -p 26379 SENTINEL replicas mymaster
redis-cli -p 26379 SENTINEL sentinels mymaster
```

---

## 4. SQLite

**When to use this section**: user is working with SQLite (embedded database, CLI `.dump`, WAL mode, VACUUM, single-user or low-concurrency applications).

### When to Use SQLite

SQLite is appropriate for:
- Embedded applications, desktop software, mobile apps
- Development/testing environments
- Low-concurrency web applications (< ~100 writes/second)
- Single-writer scenarios where simplicity outweighs scalability

Do NOT use SQLite for: high-concurrency writes, network-accessible databases, or databases requiring fine-grained user permissions.

### SQLite CLI Basics

```bash
# Open or create a database
sqlite3 myapp.db

# Useful dot-commands
.help
.tables                   # list all tables
.schema tablename         # show CREATE statement
.mode column              # column-aligned output
.headers on               # show column headers
.output results.txt       # redirect output to file
.output stdout            # restore output to terminal
.quit
```

### Backup and Restore

```bash
# Online backup (safe while database is in use)
sqlite3 myapp.db ".backup myapp_backup_$(date +%Y%m%d).db"

# Plain SQL dump (portable)
sqlite3 myapp.db .dump > myapp_$(date +%Y%m%d).sql

# Restore from SQL dump
sqlite3 myapp_restored.db < myapp_20240101.sql

# Copy-based backup (safe with WAL mode, see below)
cp myapp.db myapp_$(date +%Y%m%d).db
cp myapp.db-wal myapp_$(date +%Y%m%d).db-wal 2>/dev/null || true
cp myapp.db-shm myapp_$(date +%Y%m%d).db-shm 2>/dev/null || true
```

### VACUUM and Maintenance

```sql
-- Reclaim space after large deletes (rewrites the entire database)
VACUUM;

-- Incremental vacuum (requires auto_vacuum = INCREMENTAL)
PRAGMA incremental_vacuum(100);  -- free 100 pages

-- Analyze statistics for query planner
ANALYZE;

-- Check database integrity
PRAGMA integrity_check;

-- Show page count and page size
PRAGMA page_count;
PRAGMA page_size;
```

### WAL Mode (Write-Ahead Logging)

WAL mode dramatically improves concurrent read performance and reduces write latency:

```sql
-- Enable WAL mode (persists across connections)
PRAGMA journal_mode = WAL;

-- Verify
PRAGMA journal_mode;  -- returns "wal"

-- Tune WAL checkpoint threshold (pages before auto-checkpoint)
PRAGMA wal_autocheckpoint = 1000;
```

In WAL mode, readers do not block writers and writers do not block readers.

### Performance Tips

```sql
-- Use transactions for bulk inserts (orders of magnitude faster)
BEGIN TRANSACTION;
INSERT INTO table VALUES (...);
-- repeat thousands of times
COMMIT;

-- Tune synchronous mode (NORMAL is safe with WAL, FULL is safest)
PRAGMA synchronous = NORMAL;

-- Increase page cache
PRAGMA cache_size = -65536;  -- 64MB in pages (negative = kibibytes)

-- Use memory-mapped I/O for read-heavy workloads
PRAGMA mmap_size = 268435456;  -- 256MB

-- Create indexes on frequently queried columns
CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);

-- EXPLAIN a query to check index usage
EXPLAIN QUERY PLAN SELECT * FROM users WHERE email = 'alice@example.com';
```

---

## 5. Cross-Database Topics

### Backup Strategies

**Full backup**: complete snapshot of all data. Easiest to restore. Run daily or weekly.

**Incremental backup**: only changes since the last backup. Faster and smaller. Requires full backup + chain of incremental backups to restore.

**Point-in-time recovery (PITR)**: restore to any moment in time. Requires continuous WAL/binary log archiving.

| Strategy | PostgreSQL | MySQL/MariaDB | Redis | SQLite |
|---|---|---|---|---|
| Full | pg_dump / pg_basebackup | mysqldump / xtrabackup | BGSAVE | .backup |
| Incremental | WAL archiving | Binary logs | AOF | N/A |
| PITR | pg_basebackup + WAL archive | Full backup + binary logs | AOF | N/A |

**Backup automation example** (cron, adjust paths as needed):
```bash
# /etc/cron.d/db-backup
0 2 * * * postgres pg_dump -Fc myapp > /backup/pg/myapp_$(date +\%Y\%m\%d).dump
0 3 * * * root mysqldump -u root -p$(cat /root/.mysqlpw) myapp | gzip > /backup/mysql/myapp_$(date +\%Y\%m\%d).sql.gz
```

Always test restores periodically — a backup that cannot be restored is worthless.

### Monitoring Commands Quick Reference

```bash
# PostgreSQL
psql -U postgres -c "SELECT count(*) FROM pg_stat_activity;"
psql -U postgres -c "SELECT pg_database_size('myapp');"

# MySQL/MariaDB
mysql -u root -p -e "SHOW STATUS LIKE 'Connections';"
mysql -u root -p -e "SHOW STATUS LIKE 'Slow_queries';"

# Redis
redis-cli INFO stats | grep instantaneous_ops_per_sec
redis-cli INFO keyspace

# SQLite (check database file size)
ls -lh myapp.db
sqlite3 myapp.db "SELECT count(*) FROM sqlite_master WHERE type='table';"
```

### Connection Pooling

Direct connections are expensive. For high-throughput applications, use a connection pooler:

**PgBouncer** (PostgreSQL):
```ini
# /etc/pgbouncer/pgbouncer.ini
[databases]
myapp = host=127.0.0.1 port=5432 dbname=myapp

[pgbouncer]
listen_port = 6432
listen_addr = 127.0.0.1
auth_type = scram-sha-256
pool_mode = transaction      # transaction pooling for maximum efficiency
max_client_conn = 1000
default_pool_size = 25
```

**ProxySQL** (MySQL/MariaDB):
- Handles connection pooling, read/write splitting, query routing, and failover
- Configured via its admin interface on port 6032

**Redis connection pooling**: handled in the application layer (e.g., `redis-py`'s `ConnectionPool`, or `ioredis` in Node.js). Keep pool size proportional to number of app workers.

### Schema Migration Best Practices

1. **Always use a migration tool** — Flyway, Liquibase (Java), Alembic (Python), golang-migrate (Go), Prisma Migrate (Node.js). Never apply raw SQL changes manually in production.

2. **Each migration should be**:
   - Idempotent or version-controlled (tool handles this)
   - Backward-compatible when possible (add columns, do not drop/rename immediately)
   - Tested on a staging environment first

3. **Zero-downtime schema changes**:
   - Add new nullable column → deploy app that writes to both old and new → backfill data → add NOT NULL constraint → remove old column
   - For large tables, use `pt-online-schema-change` (MySQL) or `pg_repack` / `ALTER TABLE ... CONCURRENTLY` (PostgreSQL)

4. **Always backup before running migrations** in production:
   ```bash
   # PostgreSQL
   pg_dump -Fc myapp > pre_migration_$(date +%Y%m%d_%H%M%S).dump

   # MySQL
   mysqldump -u root -p myapp > pre_migration_$(date +%Y%m%d_%H%M%S).sql
   ```

5. **Keep migrations small and focused** — one logical change per migration file. Large, combined migrations are harder to roll back and debug.

### General Troubleshooting Checklist

When a database issue is reported, work through these in order:

1. Check service status: `systemctl status postgresql` / `mysql` / `redis`
2. Check error logs:
   - PostgreSQL: `/var/log/postgresql/postgresql-*.log`
   - MySQL: `/var/log/mysql/error.log`
   - Redis: `/var/log/redis/redis-server.log`
3. Check disk space: `df -h` — a full disk causes silent failures
4. Check open connections: compare current connections to `max_connections`
5. Check for long-running queries or lock waits (see per-database monitoring commands above)
6. Check memory pressure: `free -h`, check OOM killer in `dmesg | tail -50`
7. Review recent schema changes or application deployments that coincide with the issue onset
