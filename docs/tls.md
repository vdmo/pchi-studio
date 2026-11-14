# Sessions that survive a restart, and HTTPS

## Sessions kept in a file

By default a signed-in session lives in memory: restart the runtime and everyone has to sign in again, which is the last thing you want mid-show.

```bash
pchi-studio-api show.pchi --roster people.json --sessions sessions.json
```

- The file holds only a **SHA-256 hash of each session token**, the person's name and role, and when it ends. Reading the file does not let anyone in, and it holds no key.
- It is written whenever someone signs in or out (a temporary file renamed over the old one) and is owner-only on Linux and macOS.
- At startup a session is **dropped** if it has expired, if its person is no longer on the roster or has a different role, or if it was the operator key's and that key is gone. Change someone's role and
  restart: their old session does not keep the old role.
- A file that is not a sessions file is a startup error, not silently ignored.
- A standby (`failover.md`) keeps its own file: sessions are not carried over a takeover, people sign in again.

## HTTPS

The runtime speaks plain HTTP. On a venue network that means keys, session tokens and the feed token cross it in clear text. Put HTTPS **in front** of it with a reverse proxy, and keep the runtime on
loopback (`--bind 127.0.0.1:3100`) so only the proxy can reach it.

Caddy (gets and renews the certificate itself, WebSockets work as they are):

```
show.example.com {
    reverse_proxy 127.0.0.1:3100
}
```

nginx:

```
server {
    listen 443 ssl;
    server_name show.example.com;
    ssl_certificate     /etc/ssl/show.crt;
    ssl_certificate_key /etc/ssl/show.key;
    location / {
        proxy_pass http://127.0.0.1:3100;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;       # the live feed is a WebSocket
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 3600s;
    }
}
```

On a venue with no public name, use a certificate from your own certificate authority, or `mkcert`, and install its root on the machines that run Studio.

The runtime warns at startup when it listens beyond this machine with a roster or edit key and no HTTPS; add `--behind-tls` once the proxy is there and the warning stops. The flag changes nothing else: it
is a statement, not a switch. Studio already uses `wss://` when its page is served over HTTPS.

## What this does not cover

- Between the proxy and the runtime it is plain HTTP on loopback: that is why it must be loopback.
- The failover key travels between runtimes in a header; run the standby's connection over a network you trust or a tunnel (a VPN, SSH).
- Devices (lasers, desks) speak their own protocols, almost all unencrypted: HTTPS in front of the runtime does not change that.
