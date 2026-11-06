# Listen along relay

Production uses a Cloudflare Worker on the existing Moosik zone. Each room has its own Durable Object. It expires after 45 seconds without a host heartbeat. The invite contains only the read token. The web handoff and badge assets are served by moosik.dev.

For the local development relay, run `node listen-relay/server.mjs` with Node 20 or later. It listens on loopback port 8787 by default. Use `HOST` and `PORT` to bind behind your HTTPS endpoint. The desktop client expects `https://rooms.moosik.dev`. For local testing set `MOOSIK_LOCAL_RELAY=http://127.0.0.1:8787` in each client's launch environment.

Use one process/replica. Rooms are temporary and live in memory. Only the host gets the write token. Anyone with the invite can read the latest song and playback position. Leaving removes the room, and a missing host heartbeat expires it after 45 seconds. Restarting the relay ends all sessions. Keep response caching off, forward the Authorization header, and rate-limit room creation at the public ingress.

Run `node --test listen-relay/server.test.mjs` to check the HTTP contract. With a local relay running, run `MOOSIK_LOCAL_RELAY=http://127.0.0.1:8787 scripts/ark-cargo.sh test -p moosik --lib native_host_and_guest_follow_a_room -- --ignored` to check the native clients. In the app, enable Discord Rich Presence under Settings → System and turn on Host Listen along.

## Production abuse limits

The zone firewall rejects unsupported relay paths and methods before Worker execution, with an exception for `/.well-known/`. A separate WAF rule on `rooms.moosik.dev` blocks a source IP for 10 seconds after 80 requests in 10 seconds, before the Worker. The Worker separately admits up to 4 creation attempts and 480 room requests per minute per IP per Cloudflare location. Global quota refills are throttled per location as well.

A single SQLite-backed budget object enforces four starts per source IP in a rolling minute across locations, storing keyed address digests for recent successful admissions. It allocates at most 100 room starts and 60,000 room operations per UTC day worldwide. Playback units are reserved in batches of 32. Unused units in evicted isolates are lost, so service can reach its effective limit earlier. Over-budget requests receive HTTP 429. The daily limit covers admissions to room objects; it is not an account spending cap.

Invites keep the 32-character wire format. The Worker validates their signature and 12-hour lifetime before looking up a room, and validates the separate write token before forwarding updates. Idle rooms still expire after 45 seconds. Cloudflare holds the signing secret in the `ROOM_SIGNING_KEY` Worker secret binding. Preserve it across redeployments; rotating it invalidates existing invites. Never include its value in source or a desktop build.
