// Minimal test environment, not a claim of browser compatibility.
globalThis.window = globalThis;
globalThis.self = globalThis;
globalThis.global = globalThis;
globalThis.origin = 'https://www.youtube.com';
globalThis.location = {
    href: 'https://www.youtube.com/', origin, protocol: 'https:',
    hostname: 'www.youtube.com', host: 'www.youtube.com', pathname: '/', search: '', hash: ''
};
globalThis.navigator = {
    userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36(KHTML, like Gecko)',
    language: 'en-US', languages: ['en-US', 'en'], platform: 'MacIntel',
    hardwareConcurrency: 4, cookieEnabled: true, maxTouchPoints: 0,
    plugins: [], mimeTypes: []
};
globalThis.performance = { now: () => Date.now() - __started, timeOrigin: Date.now() };
const __started = Date.now();
globalThis.__timers = [];
let __timerId = 0;
globalThis.setTimeout = (fn, delay = 0, ...args) => {
    const id = ++__timerId;
    __timers.push({id, fn, args, at: Date.now() + Math.max(0, Number(delay) || 0)});
    return id;
};
globalThis.clearTimeout = id => { __timers = __timers.filter(t => t.id !== id); };
globalThis.setInterval = (fn, delay = 0, ...args) => {
    const id = ++__timerId;
    const repeat = Math.max(1, Number(delay) || 0);
    __timers.push({id, fn, args, at: Date.now() + repeat, repeat});
    return id;
};
globalThis.clearInterval = clearTimeout;
globalThis.setImmediate = (fn, ...args) => setTimeout(fn, 0, ...args);
globalThis.clearImmediate = clearTimeout;
globalThis.__pump = () => {
    const due = __timers.filter(t => t.at <= Date.now());
    __timers = __timers.filter(t => t.at > Date.now());
    for (const t of due) {
        if (t.repeat) __timers.push({...t, at: Date.now() + t.repeat});
        t.fn(...t.args);
    }
};
globalThis.addEventListener = () => {};
globalThis.removeEventListener = () => {};
globalThis.console = {log() {}, warn() {}, error() {}, info() {}, debug() {}};
class Element {
    constructor(name = 'DIV') {
        this.tagName = name.toUpperCase(); this.nodeName = this.tagName;
        this.style = {}; this.children = []; this.attributes = {};
    }
    appendChild(child) { this.children.push(child); child.parentNode = this; return child; }
    removeChild(child) { this.children = this.children.filter(c => c !== child); return child; }
    setAttribute(k, v) { this.attributes[k] = String(v); }
    getAttribute(k) { return this.attributes[k] ?? null; }
    addEventListener() {}
    removeEventListener() {}
    getBoundingClientRect() { return {x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, bottom: 0, right: 0}; }
}
globalThis.HTMLElement = Element;
globalThis.document = {
    createElement: name => new Element(name), createTextNode: text => ({textContent: text}),
    getElementById: () => null, querySelector: () => null, querySelectorAll: () => [],
    addEventListener() {}, removeEventListener() {},
    documentElement: new Element('HTML'), head: new Element('HEAD'), body: new Element('BODY'),
    referrer: 'https://www.youtube.com/', URL: location.href, cookie: '',
    readyState: 'complete', visibilityState: 'visible', hidden: false
};
document.defaultView = globalThis;
// Record only missing API names, never values, program bytes, or tokens.
globalThis.__missing = new Set();
function observed(name, object) {
    return new Proxy(object, {get(target, key, receiver) {
        if (typeof key === 'string' && /^[A-Za-z][A-Za-z0-9_]{0,40}$/.test(key) && !(key in target))
            __missing.add(name + '.' + key);
        return Reflect.get(target, key, receiver);
    }});
}
globalThis.document = observed('document', document);
globalThis.navigator = observed('navigator', navigator);
globalThis.screen = observed('screen', {width: 1920, height: 1080, availWidth: 1920, availHeight: 1080, colorDepth: 24, pixelDepth: 24});
globalThis.innerWidth = 1920;
globalThis.innerHeight = 1080;
globalThis.devicePixelRatio = 1;
