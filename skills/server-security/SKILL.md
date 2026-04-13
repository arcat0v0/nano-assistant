---
name: server-security
description: Server security best practices — SSH hardening, firewall (ufw/nftables), fail2ban, automatic updates, audit logging, SSL/TLS, and security checklist
version: 0.1.0
author: nano-assistant
tags: [security, ssh, firewall, ufw, nftables, fail2ban, ssl, tls, hardening, audit]
---

# Server Security Hardening

This skill covers hardening a Linux server end-to-end. Follow Section 7 (Security Checklist) for an ordered workflow on new servers.

---

## 1. SSH Hardening

Edit `/etc/ssh/sshd_config`:

```
Port 2222
PermitRootLogin no
PasswordAuthentication no
ChallengeResponseAuthentication no
AllowUsers deploy admin
ClientAliveInterval 300
ClientAliveCountMax 2
MaxAuthTries 3
X11Forwarding no
AllowAgentForwarding no
AllowTcpForwarding no
KexAlgorithms curve25519-sha256,diffie-hellman-group14-sha256
Ciphers chacha20-poly1305@openssh.com,aes256-gcm@openssh.com
MACs hmac-sha2-256-etm@openssh.com,hmac-sha2-512-etm@openssh.com
LogLevel VERBOSE
```

Validate and apply:

```bash
sshd -t                    # dry-run — fix errors before reload
systemctl reload sshd
ss -tlnp | grep sshd       # confirm new port before closing session
```

Set up key auth for a new user:

```bash
useradd -m -s /bin/bash deploy
mkdir -p /home/deploy/.ssh && chmod 700 /home/deploy/.ssh
# paste public key into authorized_keys:
nano /home/deploy/.ssh/authorized_keys
chmod 600 /home/deploy/.ssh/authorized_keys
chown -R deploy:deploy /home/deploy/.ssh
```

---

## 2. Firewall

### UFW (Debian/Ubuntu)

```bash
apt install ufw
ufw default deny incoming
ufw default allow outgoing
ufw allow 2222/tcp comment "SSH"
ufw allow 80/tcp comment "HTTP"
ufw allow 443/tcp comment "HTTPS"
ufw allow from 10.0.0.5 to any port 5432 comment "Postgres from app"
ufw enable
ufw status numbered
ufw delete 3          # remove rule by number
ufw reload
```

### nftables — `/etc/nftables.conf`

```nft
#!/usr/sbin/nft -f
flush ruleset

table inet filter {
    chain input {
        type filter hook input priority 0; policy drop;
        ct state established,related accept
        iifname "lo" accept
        ct state invalid drop
        ip protocol icmp icmp type echo-request limit rate 10/second accept
        tcp dport 2222 ct state new limit rate 15/minute accept
        tcp dport { 80, 443 } accept
        log prefix "nftables-drop: " flags all drop
    }
    chain forward { type filter hook forward priority 0; policy drop; }
    chain output  { type filter hook output  priority 0; policy accept; }
}
```

```bash
nft -c -f /etc/nftables.conf   # validate
nft -f /etc/nftables.conf      # apply
systemctl enable --now nftables
nft list ruleset
```

**iptables migration:** `iptables-save > backup.rules && iptables-restore-translate -f backup.rules > /etc/nftables-migrated.conf`. Review output carefully before applying — check custom chains, LOG targets, and REJECT vs DROP.

---

## 3. fail2ban

```bash
apt install fail2ban     # Debian
dnf install fail2ban     # Fedora/RHEL
```

Create `/etc/fail2ban/jail.local` (never edit `jail.conf`):

```ini
[DEFAULT]
bantime  = 3600
findtime = 600
maxretry = 5
destemail = admin@example.com
action = %(action_mwl)s

[sshd]
enabled  = true
port     = 2222
logpath  = /var/log/auth.log
maxretry = 3
bantime  = 86400

[nginx-http-auth]
enabled  = true
port     = http,https
logpath  = /var/log/nginx/error.log

[nginx-limit-req]
enabled  = true
port     = http,https
logpath  = /var/log/nginx/error.log
maxretry = 10
findtime = 60
bantime  = 600

[apache-auth]
enabled  = true
port     = http,https
logpath  = /var/log/apache2/error.log
```

```bash
systemctl enable --now fail2ban
fail2ban-client status           # list all jails
fail2ban-client status sshd      # jail detail
fail2ban-client set sshd banip 1.2.3.4
fail2ban-client set sshd unbanip 1.2.3.4
journalctl -u fail2ban -f
```

---

## 4. Automatic Updates

### unattended-upgrades (Debian/Ubuntu)

```bash
apt install unattended-upgrades
dpkg-reconfigure -plow unattended-upgrades
```

`/etc/apt/apt.conf.d/50unattended-upgrades` key settings:

```
Unattended-Upgrade::Remove-Unused-Dependencies "true";
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "03:00";
Unattended-Upgrade::Mail "admin@example.com";
```

`/etc/apt/apt.conf.d/20auto-upgrades`:

```
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
```

```bash
unattended-upgrade --dry-run --debug
```

### dnf-automatic (Fedora/RHEL)

```bash
dnf install dnf-automatic
# Edit /etc/dnf/automatic.conf: apply_updates = yes, upgrade_type = security
systemctl enable --now dnf-automatic-install.timer
```

---

## 5. Audit & Logging

```bash
apt install auditd audispd-plugins
systemctl enable --now auditd
```

`/etc/audit/rules.d/hardening.rules`:

```
-D
-b 8192
-f 1
-w /etc/passwd -p wa -k user-modify
-w /etc/shadow -p wa -k user-modify
-w /etc/sudoers -p wa -k sudoers-modify
-w /etc/ssh/sshd_config -p wa -k ssh-config
-a always,exit -F arch=b64 -S execve -F euid=0 -F auid!=0 -k priv-escalation
-a always,exit -F arch=b64 -S unlink -S unlinkat -F auid>=1000 -k file-delete
-w /etc/hosts -p wa -k network-config
-e 2
```

```bash
augenrules --load
ausearch -k user-modify -ts today
aureport --summary
```

journalctl filters:

```bash
journalctl -u sshd --since "1 hour ago" | grep -E "Failed|Accepted|Invalid"
journalctl _COMM=sudo --since today
journalctl SYSLOG_FACILITY=10 --since today   # all auth events
```

Enable persistent journal: `mkdir -p /var/log/journal && systemd-tmpfiles --create --prefix /var/log/journal`

---

## 6. SSL/TLS

### certbot

```bash
apt install certbot python3-certbot-nginx
certbot --nginx -d example.com -d www.example.com
certbot renew --dry-run
systemctl status certbot.timer    # auto-renewal timer
```

### nginx SSL config

```nginx
server {
    listen 443 ssl http2;
    server_name example.com;
    ssl_certificate     /etc/letsencrypt/live/example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/example.com/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384;
    ssl_prefer_server_ciphers off;
    ssl_session_cache shared:SSL:10m;
    ssl_session_timeout 1d;
    ssl_session_tickets off;
    ssl_stapling on;
    ssl_stapling_verify on;
    ssl_trusted_certificate /etc/letsencrypt/live/example.com/chain.pem;
    resolver 1.1.1.1 8.8.8.8 valid=300s;
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains; preload" always;
    add_header X-Frame-Options DENY always;
    add_header X-Content-Type-Options nosniff always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;
}
server { listen 80; server_name example.com; return 301 https://$host$request_uri; }
```

Generate DH params once: `openssl dhparam -out /etc/nginx/dhparam.pem 2048`

**acme.sh alternative:** `curl https://get.acme.sh | sh && acme.sh --issue -d example.com -w /var/www/html`, then `acme.sh --install-cert` with `--reloadcmd "systemctl reload nginx"`.

---

## 7. Security Checklist

Ordered steps for hardening a new server:

**Phase 1 — Initial OS Setup**
- [ ] `apt update && apt full-upgrade -y`
- [ ] Set hostname and timezone: `hostnamectl set-hostname srv01 && timedatectl set-timezone UTC`
- [ ] Create non-root sudo user and copy SSH public key

**Phase 2 — SSH**
- [ ] Edit `/etc/ssh/sshd_config` (change port, disable root/password auth, set AllowUsers)
- [ ] `sshd -t` then `systemctl reload sshd`
- [ ] Open new SSH port in firewall; verify login from a second terminal before closing current session

**Phase 3 — Firewall**
- [ ] Configure ufw or nftables with default-deny inbound
- [ ] Allow only required ports; enable and verify persistence

**Phase 4 — fail2ban**
- [ ] Create `jail.local` with SSH jail; add web server jails if applicable
- [ ] `systemctl enable --now fail2ban && fail2ban-client status`

**Phase 5 — Automatic Updates**
- [ ] Configure unattended-upgrades or dnf-automatic for security patches
- [ ] Verify with a dry run

**Phase 6 — SSL/TLS**
- [ ] Obtain certificates with certbot; configure nginx/apache with TLS 1.2/1.3 only
- [ ] Enable HSTS and OCSP stapling; confirm auto-renewal timer

**Phase 7 — Audit & Monitoring**
- [ ] Install auditd with hardening rules; enable persistent journal
- [ ] Run `lynis audit system` and address HIGH findings
- [ ] Check for unexpected SUID binaries and listening ports: `find / -xdev -perm /4000 -ls` and `ss -tlnp`

---

## 8. Incident Response

### Detect

```bash
last -n 50 && lastb -n 50           # recent logins and failures
who -a                              # currently logged in
ps auxf                             # process tree
ss -tlnp                            # listening ports
tail -200 /var/log/auth.log
grep "Accepted\|Failed" /var/log/auth.log | tail -100
```

### Isolate

```bash
ufw default deny incoming && ufw allow from YOUR_IP_HERE
# nftables: nft add rule inet filter input ct state new drop
```

Take a disk/VM snapshot from the provider console before any changes.

### Investigate

```bash
debsums -c 2>/dev/null              # Debian: check modified system files
rpm -Va 2>/dev/null                 # RHEL: verify installed packages
find /etc /usr /bin /sbin -newer /tmp -ls 2>/dev/null  # recently changed files
for u in $(cut -d: -f1 /etc/passwd); do crontab -u $u -l 2>/dev/null; done
cat /root/.bash_history
ausearch -ts 2025-01-01 -te 2025-01-02 -k priv-escalation
```

### Remediate

1. Revoke all SSH keys and rotate credentials for all users
2. Reset application secrets, API keys, database passwords
3. Remove unauthorized accounts or SSH keys
4. Reinstall modified system packages: `apt install --reinstall <package>`
5. Re-apply all checklist steps; change SSH port again

### Document

Write a post-mortem covering: timeline, entry point, scope of access, actions taken, and prevention controls that would have blocked the attack. Store off-server in a secure location.
