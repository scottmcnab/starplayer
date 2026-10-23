# StarPlayer remote proxy

This Cloudflare Worker serves one upstream, allows listed page origins, and caches
Hornet files at the edge. It is not an open proxy. Redirects are not followed, so
an upstream redirect cannot send the Worker to another host.

From this directory run `npx wrangler login`, then `npx wrangler deploy`. Deployment
prints `https://starplayer-hornet-proxy.<subdomain>.workers.dev`. No deployment is
needed for direct streaming from a server that already supports CORS.

The deployed instance is `https://starplayer-hornet-proxy.starplayer-hornet-proxy.workers.dev/`.

Edit `ALLOWED_ORIGINS` in `wrangler.toml` to change the comma-separated page origins.
Edit `UPSTREAM` to use another HTTP(S) archive base; it must end with `/`.

Check a deployed Worker with:

```sh
curl -I -H 'Origin: https://scottmcnab.github.io' https://starplayer-hornet-proxy.<subdomain>.workers.dev/path/to/song.s3m
```

Expect the allowed origin, cross-origin resource policy, exposed Content-Length,
and cache headers. The free tier allows 100k requests a day; Hornet files are
cached at the edge (30 days at the Worker, one year at the browser).

Put the deployed URL in `www/remote-sources.js` to prefill the player's Proxy URL.
Users can also enter it themselves. Keep the trailing `/`.
