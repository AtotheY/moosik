# Listen along relay

Production uses a Cloudflare Worker on the existing Moosik zone. Each room has its own Durable Object. It expires after 45 seconds without a host heartbeat. The invite contains only the read token. The web handoff and badge assets are served by moosik.dev.

For the local development relay, run `node listen-relay/server.mjs` with Node 20 or later. It listens on loopback port 8787 by default. Use `HOST` and `PORT` to bind behind your HTTPS endpoint. The desktop client expects `https://rooms.moosik.dev`. For local testing set `MOOSIK_LOCAL_RELAY=http://127.0.0.1:8787` in each client's launch environment.

Use one process/replica. Rooms are temporary and live in memory. Only the host gets the write token. Anyone with the invite can read the latest song and playback position. Leaving removes the room, and a missing host heartbeat expires it after 45 seconds. Restarting the relay ends all sessions. Keep response caching off, forward the Authorization header, and rate-limit room creation at the public ingress.

Run `node --test listen-relay/server.test.mjs` to check the HTTP contract. With a local relay running, run `MOOSIK_LOCAL_RELAY=http://127.0.0.1:8787 scripts/ark-cargo.sh test -p moosik --lib native_host_and_guest_follow_a_room -- --ignored` to check the native clients. In the app, enable Discord Rich Presence under Settings → System and turn on Host Listen along.
