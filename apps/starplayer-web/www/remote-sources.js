// Fill in the deployed Worker URL using remote-proxy/README.md.
export const REMOTE_SOURCES = [{ upstream: 'https://hornet.org/music/', proxy: '' }];
export function defaultProxyFor(base) { return REMOTE_SOURCES.find(source => source.upstream === base)?.proxy ?? ''; }
