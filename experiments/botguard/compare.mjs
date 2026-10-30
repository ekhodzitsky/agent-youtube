// Diagnostic reference only. The application never invokes Node or this script.
import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';

const [serverPath, probePath = 'experiments/botguard/target/release/agent-youtube-botguard-probe', video = 'a2hbJlPxj4M'] = process.argv.slice(2);
if (!serverPath || !/^[A-Za-z0-9_-]{11}$/.test(video)) {
    throw new Error('usage: node compare.mjs PROVIDER_SERVER_DIR [PROBE_BINARY] [VIDEO_ID]');
}
const deadline = setTimeout(() => process.exit(124), 120_000);
// Upstream debug/error messages can include tokens. Emit our redacted report only.
for (const method of ['log', 'debug', 'info', 'warn', 'error']) console[method] = () => {};
const {SessionManager} = await import(pathToFileURL(path.resolve(serverPath, 'build/session_manager.js')));
const {default: axios} = await import(pathToFileURL(path.resolve(serverPath, 'node_modules/axios/index.js')));
fs.mkdirSync('runs', {recursive: true});
const directory = fs.mkdtempSync('runs/botguard-comparison-');
fs.chmodSync(directory, 0o700);
const save = (name, value) => {
    const file = path.join(directory, name);
    fs.writeFileSync(file, typeof value === 'string' ? value : JSON.stringify(value), {flag: 'wx', mode: 0o600});
    return file;
};
const runProbe = (fixture, label) => {
    const result = spawnSync(probePath, [video], {
        env: {...process.env, AGENT_YOUTUBE_BG_INPUT: fixture, AGENT_YOUTUBE_BG_API: 'google'},
        encoding: 'utf8', timeout: 90_000, maxBuffer: 8 * 1024 * 1024,
    });
    save(`${label}.stderr`, result.stderr || '');
    let summary;
    try { summary = JSON.parse(result.stdout); } catch { summary = null; }
    return {exit: result.status, summary, timed_out: result.error?.code === 'ETIMEDOUT'};
};
const report = {fixture_directory: directory};
axios.interceptors.response.use(response => {
    const url = new URL(response.config.url);
    if (url.hostname === 'jnn-pa.googleapis.com' && url.pathname.endsWith('/GenerateIT')) {
        report.reference_integrity_issued = typeof response.data?.[0] === 'string' && !!response.data[0];
        const snapshot = JSON.parse(response.config.data)[1];
        report.rust_http_control = runProbe(save('snapshot.json', {snapshot}), 'http-control');
    }
    return response;
});
const manager = new SessionManager(false, {});
// Pin the provider revision in the research record: this private method is not
// an application API. Preserve its actual challenge acquisition and page context.
const getChallenge = manager.getDescrambledChallenge.bind(manager);
manager.getDescrambledChallenge = async (...args) => {
    const c = await getChallenge(...args);
    const fixture = save('challenge.json', {
        challenge: ['id', [c.interpreterJavascript.privateDoNotAccessOrElseSafeScriptWrappedValue], null, c.interpreterHash, c.program, c.globalName],
        config: globalThis.yt?.config_ || {},
    });
    report.rust_vm = runProbe(fixture, 'vm');
    return c;
};
try {
    const result = await manager.generatePoToken(video, '', true);
    report.reference_po_token_chars = result.poToken.length;
    report.media_acceptance = 'not_tested';
    process.stdout.write(`${JSON.stringify(report)}\n`);
} catch {
    process.stdout.write(`${JSON.stringify({...report, reference_failed: true})}\n`);
    process.exitCode = 1;
} finally {
    clearTimeout(deadline);
}
