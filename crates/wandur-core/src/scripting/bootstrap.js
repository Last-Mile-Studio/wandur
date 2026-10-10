// The host side of a world script, run once in each script's own QuickJS context before the
// script's source. It defines the frozen `mud` and `Events` globals and returns the dispatcher
// the host calls for every event. Ported from the C# client's JavaScriptEngine bootstrap (same
// owner, MIT), with three changes for QuickJS:
// - code generation from strings (eval, the Function constructors) is refused, as the C# engine
//   refuses it with StringCompilationAllowed = false;
// - an event that produced nothing returns null, so the host parses no JSON per quiet line;
// - a result names the script's alias count when it changed, so the host knows whether typed
//   commands must wait for the script.
// Hook ids, mud.remove and mud.after come from the C# feature/mudlet-import branch.
// __RESTRICTED_SEND__ is replaced by true or false before evaluation.
(() => {
    const aliases = [], triggers = [], timers = [], listeners = [];
    const panels = new Map(), handlers = new Map();
    const state = { gmcp: {}, msdp: {} };
    const sizes = { gmcp: { entries: new Map(), total: 0 }, msdp: { entries: new Map(), total: 0 } };
    const requested = new Set();
    const msdpName = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/, packageName = /^[A-Za-z][A-Za-z0-9_.]*$/;
    let actions = [], outputSize = 0, emitted = 0, panelCount = 0, panelSize = 0, clock = 0, hooks = 0, nextHook = 0;
    const restrictedSend = __RESTRICTED_SEND__;
    let sendAllowed = !restrictedSend;
    const stringify = JSON.stringify, parse = JSON.parse;
    let reportedAliases = 0;
    const tag = Function.call.bind(Object.prototype.toString);
    const has = Function.call.bind(Object.prototype.hasOwnProperty);
    const unsafeKey = key => key === '__proto__' || key === 'constructor' || key === 'prototype';
    function checkFunction(callback) {
        if (typeof callback !== 'function' || tag(callback) !== '[object Function]')
            throw new TypeError('Callbacks must be synchronous functions.');
    }
    function checkCallback(callback) {
        checkFunction(callback);
        if (++hooks > 256) throw new RangeError('Maximum 256 hooks per script.');
    }
    function register(list, pattern, callback) {
        if (typeof pattern !== 'string' && !(pattern instanceof RegExp))
            throw new TypeError('Pattern must be a string or RegExp.');
        const expression = new RegExp(pattern);
        if (expression.source.length > 4096) throw new RangeError('Pattern exceeds 4096 characters.');
        checkCallback(callback);
        const id = ++nextHook;
        list.push({ id, expression, callback });
        return id;
    }
    // Every hook (alias, trigger, listener, timer) has an id; removing one frees its place in the
    // budget. A hook removed while an event is being delivered does not run for the rest of it.
    function remove(id) {
        if (typeof id !== 'number') throw new TypeError('A hook id must be a number.');
        for (const list of [aliases, triggers, listeners, timers]) {
            const index = list.findIndex(entry => entry.id === id);
            if (index >= 0) { list[index].removed = true; list.splice(index, 1); hooks--; return true; }
        }
        return false;
    }
    function timer(seconds, callback, once) {
        const minimum = once ? 0 : 1;
        if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds < minimum || seconds > 2147483)
            throw new RangeError(once ? 'Delay must be 0–2147483 seconds.' : 'Timer interval must be 1–2147483 seconds.');
        checkCallback(callback);
        const id = ++nextHook;
        timers.push({ id, interval: seconds * 1000, due: clock + seconds * 1000, callback, once });
        return id;
    }
    function action(kind, text) {
        if (typeof text !== 'string') throw new TypeError('Action text must be a string.');
        if (kind === 'send') {
            if (!sendAllowed) throw new Error('Pack send policy: this script may send commands only from an alias or a panel button.');
            if (!text.length || text.length > 4096 || /[\x00-\x1f\x7f-\x9f\u2028\u2029]/.test(text))
                throw new RangeError('Commands must contain 1–4096 characters and no control characters.');
        }
        if (text.length > 8192) throw new RangeError('Echo exceeds 8192 characters.');
        if (++outputSize + text.length > 32768 || ++emitted > 32)
            throw new RangeError('Event action limit exceeded.');
        outputSize += text.length;
        actions.push({ kind, text });
    }
    function panelAction(message) {
        const text = stringify(message);
        if (text.length > 65536) throw new RangeError('A panel action exceeds 65536 characters.');
        if (++panelCount > 32) throw new RangeError('Maximum 32 panel actions per event.');
        if (panelSize + text.length > 262144) throw new RangeError('Panel output limit exceeded.');
        panelSize += text.length;
        actions.push({ kind: 'panel', text });
    }
    function identifier(value, name) {
        if (typeof value !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9_.\-]{0,63}$/.test(value))
            throw new TypeError(name + ' must be 1 to 64 characters of letters, digits, dot, dash or underscore.');
        return value;
    }
    function plain(value, name) {
        if (typeof value === 'number' && Number.isFinite(value)) value = String(value);
        else if (typeof value === 'boolean') value = String(value);
        if (typeof value !== 'string') throw new TypeError(name + ' must be a string.');
        if (value.length > 4096) throw new RangeError(name + ' exceeds 4096 characters.');
        return value;
    }
    function finite(value, name) {
        if (typeof value !== 'number' || !Number.isFinite(value)) throw new TypeError(name + ' must be a finite number.');
        return value;
    }
    function entries(value, name, limit) {
        if (!Array.isArray(value)) throw new TypeError(name + ' must be an array.');
        if (value.length > limit) throw new RangeError(name + ' exceeds ' + limit + ' items.');
        return value.map(entry => plain(entry, name));
    }
    // Display text for any value: MSDP tables arrive as objects, which would otherwise print as [object Object].
    function format(value, depth) {
        if (value === undefined || value === null) return '';
        if (typeof value === 'string') return value;
        if (typeof value === 'number' || typeof value === 'boolean' || typeof value === 'bigint') return String(value);
        if (typeof value !== 'object') return '';
        if (depth > 4) return '...';
        if (Array.isArray(value)) return value.map(item => format(item, depth + 1)).join(', ');
        return Object.keys(value).map(key => key + ': ' + format(value[key], depth + 1)).join(', ');
    }
    function barsAccepts(kind) { return kind === 'gauge' || kind === 'label'; }
    function hook(panel, widget, event, callback) {
        if (callback === undefined || callback === null) return;
        const key = panel + '\u0000' + widget + '\u0000' + event;
        if (handlers.has(key)) checkFunction(callback); else checkCallback(callback);
        handlers.set(key, callback);
    }
    function widget(record, id, kind, properties) {
        id = identifier(id, 'A widget id');
        if (properties === undefined || properties === null) properties = {};
        if (typeof properties !== 'object') throw new TypeError('Widget properties must be an object.');
        if (!record.widgets.has(id) && record.widgets.size >= 64) throw new RangeError('Maximum 64 widgets per panel.');
        if (record.dock === 'bars' && !barsAccepts(kind)) throw new TypeError('A bars panel accepts only gauge and label widgets.');
        const out = {};
        if (kind === 'gauge') {
            if (properties.label !== undefined) out.label = plain(properties.label, 'label');
            out.value = finite(properties.value === undefined ? 0 : properties.value, 'value');
            out.max = finite(properties.max === undefined ? 100 : properties.max, 'max');
            if (properties.warn !== undefined) out.warn = finite(properties.warn, 'warn');
        } else if (kind === 'label' || kind === 'text') {
            out.text = plain(properties.text === undefined ? '' : properties.text, 'text');
        } else if (kind === 'list') {
            if (properties.title !== undefined) out.title = plain(properties.title, 'title');
            out.items = entries(properties.items === undefined ? [] : properties.items, 'items', 500);
            hook(record.id, id, 'select', properties.onSelect);
        } else if (kind === 'table') {
            if (properties.title !== undefined) out.title = plain(properties.title, 'title');
            out.columns = entries(properties.columns === undefined ? [] : properties.columns, 'columns', 32);
            const rows = properties.rows === undefined ? [] : properties.rows;
            if (!Array.isArray(rows)) throw new TypeError('rows must be an array.');
            if (rows.length > 500) throw new RangeError('rows exceeds 500 items.');
            out.rows = rows.map(row => entries(row, 'a row', 32));
        } else if (kind === 'button') {
            out.label = plain(properties.label === undefined ? id : properties.label, 'label');
            hook(record.id, id, 'click', properties.onClick);
        } else if (kind === 'toggle') {
            out.label = plain(properties.label === undefined ? id : properties.label, 'label');
            out.value = properties.value === true;
            hook(record.id, id, 'change', properties.onChange);
        } else if (kind === 'input') {
            if (properties.placeholder !== undefined) out.placeholder = plain(properties.placeholder, 'placeholder');
            if (properties.value !== undefined) out.value = plain(properties.value, 'value');
            hook(record.id, id, 'submit', properties.onSubmit);
        } else if (kind === 'group') {
            if (properties.title !== undefined) out.title = plain(properties.title, 'title');
            out.children = entries(properties.children === undefined ? [] : properties.children, 'children', 64);
        }
        record.widgets.set(id, kind);
        panelAction({ panel: record.id, action: 'widget', widget: id, kind, props: out });
        return record.api;
    }
    function panel(id, options) {
        id = identifier(id, 'A panel id');
        if (options === undefined || options === null) options = {};
        if (typeof options !== 'object') throw new TypeError('Panel options must be an object.');
        const existing = panels.get(id);
        const title = options.title === undefined ? (existing === undefined ? id : existing.title) : plain(options.title, 'title');
        const dock = options.dock === undefined ? (existing === undefined ? 'right' : existing.dock) : options.dock;
        if (dock !== 'left' && dock !== 'right' && dock !== 'bars') throw new TypeError('A panel docks to left, right or bars.');
        if (existing !== undefined) {
            if (dock === 'bars' && existing.dock !== dock)
                for (const kind of existing.widgets.values())
                    if (!barsAccepts(kind)) throw new TypeError('A bars panel accepts only gauge and label widgets.');
            if (existing.title !== title || existing.dock !== dock) {
                existing.title = title; existing.dock = dock;
                panelAction({ panel: id, action: 'create', title, dock });
            }
            return existing.api;
        }
        if (panels.size >= 8) throw new RangeError('Maximum 8 panels per script.');
        const record = { id, title, dock, widgets: new Map(), api: null };
        panels.set(id, record);
        panelAction({ panel: id, action: 'create', title, dock });
        const kinds = ['gauge', 'label', 'text', 'list', 'table', 'button', 'toggle', 'input', 'separator', 'group'];
        const api = {};
        for (const kind of kinds) api[kind] = (widgetId, properties) => widget(record, widgetId, kind, properties);
        api.remove = widgetId => {
            widgetId = identifier(widgetId, 'A widget id');
            record.widgets.delete(widgetId);
            for (const event of ['click', 'change', 'submit', 'select'])
                handlers.delete(id + '\u0000' + widgetId + '\u0000' + event);
            panelAction({ panel: id, action: 'remove', widget: widgetId });
            return record.api;
        };
        api.show = options => {
            if (options !== undefined && options !== null && typeof options !== 'object') throw new TypeError('Show options must be an object.');
            panelAction({ panel: id, action: 'show' });
            if (options !== undefined && options !== null && options.focus === true) panelAction({ panel: id, action: 'focus' });
            return record.api;
        };
        api.hide = () => { panelAction({ panel: id, action: 'hide' }); return record.api; };
        // Brings the tool's tab to the front; the client accepts one focus per panel per second.
        api.focus = () => { panelAction({ panel: id, action: 'focus' }); return record.api; };
        api.close = () => {
            panels.delete(id);
            for (const key of Array.from(handlers.keys())) if (key.indexOf(id + '\u0000') === 0) handlers.delete(key);
            panelAction({ panel: id, action: 'close' });
        };
        record.api = Object.freeze(api);
        return record.api;
    }
    function store(bucket, key, value) {
        if (unsafeKey(key)) return undefined;
        let text;
        try { text = stringify(value === undefined ? null : value); } catch (_) { return undefined; }
        if (typeof text !== 'string' || text.length > 32768) return undefined;
        const bin = sizes[bucket];
        const previous = bin.entries.has(key) ? bin.entries.get(key) : 0;
        if (!bin.entries.has(key) && bin.entries.size >= 512) return undefined;
        if (bin.total - previous + text.length > 262144) return undefined;
        bin.total += text.length - previous;
        bin.entries.set(key, text.length);
        return parse(text);
    }
    function recordGmcp(name, data) {
        const parts = name.split('.');
        if (parts.length > 8 || parts.some(unsafeKey)) return;
        const copy = store('gmcp', name, data);
        if (copy === undefined) return;
        let node = state.gmcp;
        for (let i = 0; i < parts.length - 1; i++) {
            const child = node[parts[i]];
            if (child === null || typeof child !== 'object' || Array.isArray(child)) node[parts[i]] = {};
            node = node[parts[i]];
        }
        node[parts[parts.length - 1]] = copy;
    }
    function recordMsdp(variable, value) {
        const copy = store('msdp', variable, value);
        if (copy === undefined) return;
        state.msdp[variable] = copy;
    }
    // The host's cache of everything received so far, applied through the live paths so the same
    // limits and copying hold. No listener runs: a seed is not new data from the world.
    function seed(text) {
        let parsed;
        try { parsed = parse(text); } catch (_) { return; }
        if (parsed === null || typeof parsed !== 'object') return;
        const gmcp = parsed.gmcp, msdp = parsed.msdp;
        if (gmcp !== null && typeof gmcp === 'object' && !Array.isArray(gmcp))
            for (const name of Object.keys(gmcp)) if (packageName.test(name)) recordGmcp(name, gmcp[name]);
        if (msdp !== null && typeof msdp === 'object' && !Array.isArray(msdp))
            for (const name of Object.keys(msdp)) recordMsdp(name, msdp[name]);
    }
    // A variable the world has not sent is asked for once per script; the answer arrives as Events.Msdp.
    function report(name) {
        if (!msdpName.test(name) || requested.has(name) || requested.size >= 64) return;
        requested.add(name);
        actions.push({ kind: 'report', text: name });
    }
    function call(callback, argument) {
        const result = callback(argument);
        if (result && typeof result.then === 'function')
            throw new TypeError('Async callbacks are not supported.');
    }
    const refuse = function () { throw new EvalError('Code generation from strings is not allowed.'); };
    for (const prototype of [Function.prototype, Object.getPrototypeOf(function* () {}),
        Object.getPrototypeOf(async function () {}), Object.getPrototypeOf(async function* () {})])
        Object.defineProperty(prototype, 'constructor', { value: refuse, writable: false, configurable: false });
    Object.defineProperty(globalThis, 'Function', { value: refuse, writable: false, configurable: false });
    Object.defineProperty(globalThis, 'eval', { value: refuse, writable: false, configurable: false });
    Object.defineProperty(globalThis, 'Events', { value: Object.freeze({ Line: 'line', Prompt: 'prompt', Gmcp: 'gmcp', Key: 'key', Msdp: 'msdp' }), writable: false, configurable: false });
    Object.defineProperty(globalThis, 'mud', { value: Object.freeze({
        send: text => action('send', text),
        echo: text => action('echo', text),
        alias: (pattern, callback) => register(aliases, pattern, callback),
        trigger: (pattern, callback) => register(triggers, pattern, callback),
        on: (event, callback) => {
            if (event !== 'line' && event !== 'prompt' && event !== 'gmcp' && event !== 'key' && event !== 'msdp')
                throw new TypeError('Event must be line, prompt, gmcp, msdp or key.');
            checkCallback(callback);
            const id = ++nextHook;
            listeners.push({ id, event, callback });
            return id;
        },
        every: (seconds, callback) => timer(seconds, callback, false),
        after: (seconds, callback) => timer(seconds, callback, true),
        remove: id => remove(id),
        panel: (id, options) => panel(id, options),
        format: value => { const text = format(value, 1); return text.length > 4096 ? text.slice(0, 4096) : text; },
        state: Object.freeze({
            get: path => {
                if (typeof path !== 'string') throw new TypeError('A state path must be a string.');
                if (path.length > 512) throw new RangeError('A state path exceeds 512 characters.');
                const parts = path.split('.');
                if (parts[0] === 'msdp' && parts.length > 1 && !has(state.msdp, parts[1])) report(parts[1]);
                let node = state;
                for (const part of parts) {
                    if (node === null || typeof node !== 'object' || !has(node, part)) return undefined;
                    node = node[part];
                }
                return node === null || typeof node !== 'object' ? node : parse(stringify(node));
            },
            snapshot: () => parse(stringify(state))
        })
    }) });
    return (kind, text, elapsed) => {
        let handled = false;
        if (kind !== 'flush') { actions = []; outputSize = 0; emitted = 0; panelCount = 0; panelSize = 0; }
        clock = Math.max(clock, elapsed);
        let event = null, message = null;
        if (kind === 'line' || kind === 'prompt' || kind === 'key') event = Object.freeze({ text });
        else if (kind === 'gmcp') {
            const match = /^([A-Za-z][A-Za-z0-9_.]*)(?:\s+([\s\S]*))?$/.exec(text);
            if (match) {
                try {
                    const data = match[2] ? parse(match[2]) : null;
                    recordGmcp(match[1], data);
                    event = { package: match[1], data };
                }
                catch (_) { /* Malformed server data is not a script failure. */ }
            }
        } else if (kind === 'msdp') {
            try {
                const parsed = parse(text);
                if (parsed !== null && typeof parsed === 'object' && typeof parsed.variable === 'string') {
                    const value = parsed.value === undefined ? null : parsed.value;
                    recordMsdp(parsed.variable, value);
                    event = Object.freeze({ variable: parsed.variable, value });
                }
            }
            catch (_) { /* Malformed server data is not a script failure. */ }
        } else if (kind === 'panel') {
            try {
                const parsed = parse(text);
                if (parsed !== null && typeof parsed === 'object') message = parsed;
            }
            catch (_) { /* A malformed callback message is ignored. */ }
        } else if (kind === 'state') seed(text);
        sendAllowed = !restrictedSend || kind === 'command' || (message !== null && message.event === 'click');
        if (event !== null) {
            for (const listener of listeners.slice()) {
                if (listener.event === kind && !listener.removed) call(listener.callback, event);
            }
        }
        if (kind === 'command' || kind === 'line') {
            const list = (kind === 'command' ? aliases : triggers).slice();
            for (const entry of list) {
                if (entry.removed) continue;
                entry.expression.lastIndex = 0;
                const match = entry.expression.exec(text);
                if (match) {
                    call(entry.callback, match);
                    if (kind === 'command') { handled = true; break; }
                }
            }
        } else if (kind === 'tick') {
            for (const timer of timers.slice()) {
                if (!timer.removed && clock >= timer.due) {
                    timer.due = clock + timer.interval;
                    // A one-shot timer is gone before it runs, so a failing callback cannot fire it twice.
                    if (timer.once) remove(timer.id);
                    call(timer.callback);
                }
            }
        } else if (kind === 'panel') {
            if (message !== null) {
                const handler = handlers.get(message.panel + '\u0000' + message.widget + '\u0000' + message.event);
                if (handler !== undefined) call(handler, message.value === undefined ? null : message.value);
            }
        } else if (kind !== 'flush' && kind !== 'prompt' && kind !== 'gmcp' && kind !== 'key' && kind !== 'msdp' && kind !== 'state') throw new TypeError('Unknown script event.');
        let result = null;
        if (handled || actions.length > 0 || aliases.length !== reportedAliases) {
            reportedAliases = aliases.length;
            result = stringify({ handled, actions, aliases: aliases.length });
        }
        actions = []; outputSize = 0; emitted = 0; panelCount = 0; panelSize = 0;
        return result;
    };
})()
