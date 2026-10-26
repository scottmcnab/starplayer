import { startLibraryWorker } from './library-worker-jobs.js';

await startLibraryWorker({
    scope: self,
    async loadLoader() {
        const loader = await import(new URL('./starplayer_web.js', import.meta.url));
        await loader.default({ module_or_path: new URL('./starplayer_web_bg.wasm', import.meta.url) });
        return loader;
    },
});
