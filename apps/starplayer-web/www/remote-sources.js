// Default Proxy URL per upstream; the Worker is deployed from remote-proxy/ (see its README).
export const REMOTE_SOURCES = [{ upstream: 'https://hornet.org/music/', proxy: 'https://starplayer-hornet-proxy.starplayer-hornet-proxy.workers.dev/' }];
export function defaultProxyFor(base) { return REMOTE_SOURCES.find(source => source.upstream === base)?.proxy ?? ''; }
