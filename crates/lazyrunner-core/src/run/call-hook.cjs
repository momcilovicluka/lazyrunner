// lazyrunner's stack map (K): a line for each call this process makes to a port on this machine - a dev server's
// proxy to an API - which lazyrunner reads (and keeps out of the app's column). Loaded with NODE_OPTIONS=--require
// for the npm apps lazyrunner starts; it only listens, through Node's own diagnostics channels.
'use strict';
try {
  const { channel } = require('node:diagnostics_channel');
  const local = /^(localhost|127\.0\.0\.1|::1|\[::1\]|0\.0\.0\.0)$/i;
  const started = new WeakMap();
  const took = (request) => Number(process.hrtime.bigint() - started.get(request)) / 1e6;
  const url = (request, port) => {
    const host = request.host.includes(':') && !request.host.startsWith('[') ? `[${request.host}]` : request.host;
    return `${request.protocol || 'http:'}//${host}${port ? `:${port}` : ''}${request.path}`;
  };
  const say = (request, port, status) => {
    try {
      process.stdout.write(`lazyrunner-call ${request.method} ${url(request, port)} ${status} ${Math.round(took(request))}\n`);
    } catch {}
    started.delete(request);
  };
  channel('http.client.request.start').subscribe(({ request }) => {
    if (local.test(request.host || '')) started.set(request, process.hrtime.bigint());
  });
  channel('http.client.response.finish').subscribe(({ request, response }) => {
    if (!started.has(request)) return;
    const socket = response.socket || request.socket;
    say(request, socket && socket.remotePort, response.statusCode);
  });
  // localhost is tried as ::1 and 127.0.0.1: a refused call is an AggregateError, its port in each attempt's error.
  const portOf = (error) =>
    error && (error.port || (Array.isArray(error.errors) && error.errors.map((each) => each && each.port).find(Boolean)));
  channel('http.client.request.error').subscribe(({ request, error }) => {
    if (!started.has(request)) return;
    say(request, portOf(error) || (request.socket && request.socket.remotePort), 'error');
  });
} catch {}
// lazyrunner's profiles (#83): when lazyrunner asks this process - a request file named for its pid beside this
// one - it samples its own CPU with Node's inspector for the seconds asked and writes the profile where it says.
try {
  const fs = require('node:fs');
  const path = require('node:path');
  const asking = path.join(__dirname, `profile-request.${process.pid}.json`);
  const timer = setInterval(() => {
    if (!fs.existsSync(asking)) return;
    let asked;
    try {
      asked = JSON.parse(fs.readFileSync(asking, 'utf8'));
      fs.unlinkSync(asking);
    } catch {
      return;
    }
    const inspector = require('node:inspector');
    const session = new inspector.Session();
    session.connect();
    session.post('Profiler.enable', () =>
      session.post('Profiler.start', () =>
        setTimeout(() =>
          session.post('Profiler.stop', (error, result) => {
            try {
              fs.writeFileSync(`${asked.out}.part`, JSON.stringify(error ? { error: String(error) } : result.profile));
              fs.renameSync(`${asked.out}.part`, asked.out);
            } catch {}
            session.disconnect();
          }), asked.seconds * 1000)));
  }, 1000);
  if (timer.unref) timer.unref();
} catch {}
// lazyrunner's faults on this app's calls (#105): what the stack map (K, then f) sets - read from faults.json beside
// this file as it changes - delays, drops, fails or mocks the matching calls, through http, https and fetch. A failed
// or mocked call goes to lazyrunner's fault server, which answers it.
try {
  const fs = require('node:fs');
  const path = require('node:path');
  const net = require('node:net');
  const tls = require('node:tls');
  const http = require('node:http');
  const https = require('node:https');
  const file = path.join(__dirname, 'faults.json');
  const app = process.env.LAZYRUNNER_APP || '';
  const loopback = (host) => /^(localhost|127\.0\.0\.1|::1|\[::1\])$/i.test(String(host || ''));
  let read = -1;
  let faults = [];
  let port = 0;
  const load = () => {
    let stat;
    try {
      stat = fs.statSync(file);
    } catch {
      faults = [];
      read = -1;
      return;
    }
    if (stat.mtimeMs === read) return;
    try {
      const parsed = JSON.parse(fs.readFileSync(file, 'utf8'));
      faults = (parsed.faults || []).filter((fault) => fault.caller === app);
      port = parsed.port;
      read = stat.mtimeMs;
    } catch {}
  };
  const targets = (target, host, at) => {
    const colon = target.lastIndexOf(':');
    const wanted = colon > 0 ? target.slice(0, colon) : target;
    const wantedPort = colon > 0 ? target.slice(colon + 1) : '';
    const same = wanted.toLowerCase() === String(host).toLowerCase() || (loopback(wanted) && loopback(host));
    return same && (!wantedPort || wantedPort === String(at));
  };
  // Each matching fault rolls its own share: of two on the same calls, the second can strike where the first didn't.
  const find = (host, at, route) => {
    load();
    return faults.find(
      (each) =>
        targets(each.target, host, at) &&
        route.toLowerCase().startsWith(String(each.path || '').toLowerCase()) &&
        Math.random() * 100 < each.share,
    );
  };
  // A delay's wait: its ms, or somewhere up to its ms_max (a slow network).
  const waitOf = (fault) => (fault.ms_max > fault.ms ? fault.ms + Math.random() * (fault.ms_max - fault.ms) : fault.ms);
  const dropped = () =>
    Object.assign(new Error('lazyrunner dropped this call: a fault on its stack map (K)'), { code: 'ECONNRESET' });
  for (const [module, secure] of [[http, false], [https, true]]) {
    const original = module.request;
    module.request = function request(...args) {
      let options;
      let rest;
      if (typeof args[0] === 'string' || args[0] instanceof URL) {
        const url = new URL(args[0]);
        const extra = args[1] && typeof args[1] === 'object' ? args[1] : {};
        options = { protocol: url.protocol, hostname: url.hostname, port: url.port, path: url.pathname + url.search, ...extra };
        rest = args.slice(extra === args[1] ? 2 : 1);
      } else {
        options = { ...(args[0] || {}) };
        rest = args.slice(1);
      }
      const host = String(options.hostname || options.host || 'localhost').replace(/^\[|\]$/g, '');
      const at = options.port || (secure ? 443 : 80);
      const fault = find(host, at, String(options.path || '/').split('?')[0]);
      if (!fault) return original.apply(this, args);
      // No agent and a connection of lazyrunner's: Node waits for it, so it can come late, never, or elsewhere.
      const through = (connect, extra = {}) => original.call(this, { ...options, ...extra, agent: undefined, createConnection: connect }, ...rest);
      if (fault.kind === 'delay') {
        const real = secure ? tls.connect : net.createConnection;
        // Marked: the .NET app it goes to doesn't fault it again as a browser app's call (a dev server's proxy).
        const marked = Array.isArray(options.headers)
          ? {}
          : { headers: { ...(options.headers || {}), 'x-lazyrunner-faulted': String(fault.id) } };
        return through((opts, done) => {
          setTimeout(() => {
            try {
              done(null, real(opts));
            } catch (error) {
              done(error);
            }
          }, waitOf(fault));
        }, marked);
      }
      if (fault.kind === 'drop') return through((opts, done) => process.nextTick(() => done(dropped())));
      const headers = { ...(options.headers || {}), 'x-lazyrunner-fault': String(fault.id) };
      return through(() => net.createConnection({ host: '127.0.0.1', port }), { headers });
    };
    module.get = function get(...args) {
      const request = module.request(...args);
      request.end();
      return request;
    };
  }
  if (typeof globalThis.fetch === 'function') {
    const originalFetch = globalThis.fetch;
    globalThis.fetch = async function fetch(input, init) {
      let url;
      try {
        url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url);
      } catch {
        return originalFetch(input, init);
      }
      const at = url.port || (url.protocol === 'https:' ? 443 : 80);
      const fault = find(url.hostname.replace(/^\[|\]$/g, ''), at, url.pathname);
      if (!fault) return originalFetch(input, init);
      if (fault.kind === 'delay') {
        await new Promise((done) => setTimeout(done, waitOf(fault)));
        return originalFetch(input, init);
      }
      if (fault.kind === 'drop') throw new TypeError('fetch failed', { cause: dropped() });
      return originalFetch(`http://127.0.0.1:${port}/__lazyrunner_fault/${fault.id}`);
    };
  }
  try {
    require('node:module').syncBuiltinESMExports();
  } catch {}
} catch {}
// lazyrunner's browser errors (#112): with "browserErrors" on, the HTML pages this dev server serves load a small
// script of lazyrunner's that reports what goes wrong in the page - an uncaught error, an unhandled rejection,
// console.error - back to this server, which writes each as a lazyrunner-browser line lazyrunner reads into the app's
// column. Only node:http and node:https servers are touched (not node:http2): their navigations' HTML gets the script
// tag, the script and its reports are answered here, and every other request reaches the app as it came.
try {
  if (process.env.LAZYRUNNER_BROWSER_ERRORS === '1') {
    const http = require('node:http');
    const https = require('node:https');
    const SCRIPT_PATH = '/__lazyrunner/console.js';
    const REPORT_PATH = '/__lazyrunner/console';
    const TAG = `<script src="${SCRIPT_PATH}"></script>`;
    const MOST_BODY = 16 * 1024;
    const MOST_HTML = 8 * 1024 * 1024;
    const MOST_MESSAGE = 1000;
    const MOST_STACK = 4000;
    const MOST_PAGE = 500;
    const MOST_COUNT = 1000000;
    // Under lazyrunner's longest output line, whatever the text.
    const MOST_LINE = 12 * 1024;
    const LEVELS = ['error', 'rejection', 'console', 'dropped'];
    // The page's side, sent as it is written here: ES5, it never throws, and it sends only what an error says - its
    // message, its stack, its kind and the page's address without its query or hash (they can hold tokens), never
    // cookies, storage or form data. The same message again within 2 s is sent once more with how many times it
    // came; past 20 reports in 10 s, one line says how many more weren't sent.
    function pageScript(endpoint) {
      if (window.__lazyrunnerErrors) return;
      window.__lazyrunnerErrors = true;
      var SAME_WITHIN = 2000;
      var RATE_WINDOW = 10000;
      var MOST_SENT = 20;
      var MOST_MESSAGE = 1000;
      var MOST_STACK = 4000;
      var waiting = {};
      var sent = 0;
      var dropped = 0;
      var windowStart = 0;
      var dropTimer = null;
      var busy = false;
      function page() {
        return String(location.href).split(/[?#]/)[0];
      }
      function cut(text, most) {
        text = String(text == null ? '' : text);
        return text.length > most ? text.slice(0, most) : text;
      }
      function describe(value) {
        if (typeof value === 'string') return value;
        if (value instanceof Error) return String(value.name || 'Error') + ': ' + value.message;
        try {
          var json = JSON.stringify(value);
          return json === undefined ? String(value) : json;
        } catch (e) {
          return String(value);
        }
      }
      function stackOf(text) {
        var lines = String(text || '').split('\n');
        for (var i = 0; i < lines.length; i++) lines[i] = lines[i].replace(/\?[^\s()]*?(?=:\d+(?::\d+)?\)?\s*$)/, '');
        return cut(lines.join('\n'), MOST_STACK);
      }
      function post(report) {
        var body = JSON.stringify(report);
        try {
          if (navigator.sendBeacon && navigator.sendBeacon(endpoint, body)) return;
        } catch (e) {}
        try {
          if (window.fetch) {
            window.fetch(endpoint, { method: 'POST', body: body, keepalive: true, credentials: 'omit' })['catch'](function () {});
          }
        } catch (e) {}
      }
      function send(report) {
        var now = Date.now();
        if (now - windowStart >= RATE_WINDOW) {
          windowStart = now;
          sent = 0;
        }
        if (sent < MOST_SENT) {
          sent++;
          post(report);
          return;
        }
        dropped++;
        if (dropTimer) return;
        dropTimer = setTimeout(function () {
          dropTimer = null;
          var count = dropped;
          dropped = 0;
          windowStart = Date.now();
          sent = 1;
          post({ level: 'dropped', message: '', count: count, page: page() });
        }, Math.max(0, windowStart + RATE_WINDOW - now));
      }
      function report(level, message, stack) {
        if (busy) return;
        busy = true;
        try {
          message = cut(message, MOST_MESSAGE);
          if (message.indexOf('/__lazyrunner/') >= 0) return;
          var key = level + '\n' + message;
          var known = waiting[key];
          if (known) {
            known.count++;
            return;
          }
          var entry = (waiting[key] = { count: 0 });
          setTimeout(function () {
            delete waiting[key];
            if (entry.count > 0) send({ level: level, message: message, count: entry.count, again: true, page: page() });
          }, SAME_WITHIN);
          send({ level: level, message: message, stack: stackOf(stack), count: 1, page: page() });
        } catch (e) {
        } finally {
          busy = false;
        }
      }
      function said(args) {
        var parts = [];
        var next = 0;
        if (typeof args[0] === 'string' && /%[sdifoOc]/.test(args[0])) {
          next = 1;
          parts.push(
            args[0].replace(/%([sdifoOc])/g, function (all, kind) {
              if (next >= args.length) return all;
              var value = args[next++];
              return kind === 'c' ? '' : describe(value);
            })
          );
        }
        for (; next < args.length; next++) parts.push(cut(describe(args[next]), MOST_MESSAGE));
        return parts.join(' ');
      }
      function errorStack(args) {
        for (var i = 0; i < args.length; i++) if (args[i] instanceof Error && args[i].stack) return args[i].stack;
        return '';
      }
      try {
        var original = console.error;
        console.error = function () {
          try {
            report('console', said(arguments), errorStack(arguments));
          } catch (e) {}
          return original.apply(this, arguments);
        };
      } catch (e) {}
      window.addEventListener('error', function (event) {
        try {
          var error = event.error;
          var where = event.filename ? '    at ' + event.filename + ':' + event.lineno + ':' + event.colno : '';
          report('error', event.message || describe(error), (error && error.stack) || where);
        } catch (e) {}
      });
      window.addEventListener('unhandledrejection', function (event) {
        try {
          var reason = event.reason;
          report('rejection', describe(reason), reason && reason.stack);
        } catch (e) {}
      });
    }
    const SCRIPT = `(${pageScript})(${JSON.stringify(REPORT_PATH)});\n`;
    // Text from a page as it's kept: a string, at most so long, no half of a character cut in two.
    const bounded = (text, most) => {
      const kept = typeof text === 'string' ? (text.length > most ? text.slice(0, most) : text) : '';
      return kept.replace(/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g, '');
    };
    // JSON in ASCII: what lazyrunner reads is as long as it looks, however the console's code page reads it.
    const ascii = (value) =>
      JSON.stringify(value).replace(/[\u007f-￿]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);
    const tell = (sent) => {
      if (!sent || typeof sent !== 'object') return;
      const line = {
        level: LEVELS.includes(sent.level) ? sent.level : 'error',
        message: bounded(sent.message, MOST_MESSAGE),
        stack: bounded(sent.stack, MOST_STACK),
        page: bounded(sent.page, MOST_PAGE).split(/[?#]/)[0],
        count: Number.isInteger(sent.count) && sent.count > 0 ? Math.min(sent.count, MOST_COUNT) : 1,
      };
      if (sent.again === true) line.again = true;
      let text = ascii(line);
      for (const field of ['stack', 'message', 'page']) {
        while (text.length > MOST_LINE && line[field]) {
          line[field] = bounded(line[field], Math.floor(line[field].length / 2));
          text = ascii(line);
        }
      }
      process.stdout.write(`lazyrunner-browser ${text}\n`);
    };
    const answer = (response, status, headers, body) => {
      try {
        response.writeHead(status, { 'Cache-Control': 'no-store', ...headers });
        response.end(body);
      } catch {}
    };
    // Only the page's own reports: not another site's, sent to this server from elsewhere.
    const sameOrigin = (request) => {
      if (String(request.headers['sec-fetch-site'] || 'same-origin') === 'cross-site') return false;
      const origin = request.headers.origin;
      if (origin === undefined) return true;
      try {
        return new URL(origin).host === String(request.headers.host || '');
      } catch {
        return false;
      }
    };
    const receive = (request, response) => {
      if (request.method !== 'POST' || !sameOrigin(request)) {
        request.resume();
        answer(response, 403, {}, '');
        return;
      }
      const chunks = [];
      let size = 0;
      request.on('data', (chunk) => {
        size += chunk.length;
        if (size <= MOST_BODY) chunks.push(chunk);
      });
      request.on('end', () => {
        if (size > MOST_BODY) {
          answer(response, 413, {}, '');
          return;
        }
        answer(response, 204, {}, '');
        try {
          tell(JSON.parse(Buffer.concat(chunks).toString('utf8')));
        } catch {}
      });
      request.on('error', () => {});
    };
    const serve = (request, response) => {
      const known = request.method === 'GET' || request.method === 'HEAD';
      const headers = { 'Content-Type': 'text/javascript; charset=utf-8', 'X-Content-Type-Options': 'nosniff' };
      answer(response, known ? 200 : 405, headers, request.method === 'GET' ? SCRIPT : '');
    };
    // A header from writeHead's own: an object, [name, value] pairs, or names and values one after the other.
    const headerIn = (headers, name) => {
      if (!headers || typeof headers !== 'object') return undefined;
      if (Array.isArray(headers)) {
        for (let at = 0; at < headers.length; at += Array.isArray(headers[0]) ? 1 : 2) {
          const [key, value] = Array.isArray(headers[0]) ? headers[at] : [headers[at], headers[at + 1]];
          if (String(key).toLowerCase() === name) return value;
        }
        return undefined;
      }
      const key = Object.keys(headers).find((each) => each.toLowerCase() === name);
      return key === undefined ? undefined : headers[key];
    };
    const withoutLength = (headers) => {
      const length = (name) => String(name).toLowerCase() === 'content-length';
      if (!Array.isArray(headers)) return Object.fromEntries(Object.entries(headers).filter(([name]) => !length(name)));
      if (Array.isArray(headers[0])) return headers.filter(([name]) => !length(name));
      const kept = [];
      for (let at = 0; at + 1 < headers.length; at += 2) {
        if (!length(headers[at])) kept.push(headers[at], headers[at + 1]);
      }
      return kept;
    };
    // The page with the script's tag right after its <head> (or its <body>); as it was when it has neither, isn't
    // UTF-8 or has the tag already.
    const withScript = (body) => {
      const text = body.toString('utf8');
      if (text.includes(SCRIPT_PATH) || !Buffer.from(text, 'utf8').equals(body)) return body;
      const found = /<head(?:\s[^>]*)?>/i.exec(text) || /<body(?:\s[^>]*)?>/i.exec(text);
      if (!found) return body;
      const at = found.index + found[0].length;
      return Buffer.from(text.slice(0, at) + TAG + text.slice(at), 'utf8');
    };
    // A navigation's response: held until it ends when it's an uncompressed HTML page (its Content-Length dropped,
    // since the page grows), else passed on untouched.
    const watch = (response) => {
      const writeHead = response.writeHead;
      const write = response.write;
      const end = response.end;
      let state = 'waiting';
      let size = 0;
      const chunks = [];
      const decide = (status, headers) => {
        const type = String(headerIn(headers, 'content-type') || response.getHeader('content-type') || '');
        const coding = String(headerIn(headers, 'content-encoding') || response.getHeader('content-encoding') || 'identity');
        const page = /^\s*text\/html\s*(;|$)/i.test(type) && !/charset\s*=\s*"?(?!utf-?8)/i.test(type);
        const whole = status >= 200 && status < 300 && status !== 204 && coding.trim().toLowerCase() === 'identity';
        state = page && whole ? 'holding' : 'passing';
        if (state === 'holding') response.removeHeader('content-length');
      };
      const decideNow = (status) => {
        try {
          decide(status);
        } catch {
          state = 'passing';
        }
      };
      const keep = (chunk, encoding) => {
        const text = typeof chunk === 'string';
        const buffer = text ? Buffer.from(chunk, typeof encoding === 'string' ? encoding : 'utf8') : Buffer.from(chunk);
        chunks.push(buffer);
        size += buffer.length;
      };
      response.writeHead = function lazyrunnerWriteHead(status, ...rest) {
        if (state === 'waiting') {
          try {
            const at = rest.findIndex((each) => each && typeof each === 'object');
            decide(status, at >= 0 ? rest[at] : undefined);
            if (state === 'holding' && at >= 0) rest[at] = withoutLength(rest[at]);
          } catch {
            state = 'passing';
          }
        }
        return writeHead.call(this, status, ...rest);
      };
      response.write = function lazyrunnerWrite(chunk, encoding, done) {
        if (state === 'waiting') decideNow(this.statusCode);
        if (state !== 'holding') return write.apply(this, arguments);
        try {
          keep(chunk, encoding);
        } catch {
          state = 'passing';
          return write.apply(this, arguments);
        }
        if (size > MOST_HTML) {
          state = 'passing';
          write.call(this, Buffer.concat(chunks.splice(0)));
        }
        const callback = typeof encoding === 'function' ? encoding : done;
        if (typeof callback === 'function') process.nextTick(callback);
        return true;
      };
      response.end = function lazyrunnerEnd(chunk, encoding, done) {
        if (state === 'waiting') decideNow(this.statusCode);
        if (state !== 'holding') return end.apply(this, arguments);
        state = 'passing';
        let callback = done;
        let body = null;
        try {
          if (typeof chunk === 'function') {
            callback = chunk;
          } else {
            if (typeof encoding === 'function') callback = encoding;
            if (chunk !== undefined && chunk !== null) keep(chunk, encoding);
          }
          body = Buffer.concat(chunks);
          body = withScript(body);
        } catch {
          body = body || Buffer.concat(chunks);
        }
        return end.call(this, body, callback);
      };
    };
    // A page the browser navigates to: its HTML comes uncompressed, so the tag can go in.
    const navigation = (request) =>
      request.method === 'GET' && /text\/html/i.test(String(request.headers.accept || '')) && !request.headers.upgrade;
    for (const Server of [http.Server, https.Server]) {
      const proto = Server && Server.prototype;
      if (!proto || proto.lazyrunnerBrowserErrors) continue;
      Object.defineProperty(proto, 'lazyrunnerBrowserErrors', { value: true });
      const emit = proto.emit;
      proto.emit = function lazyrunnerEmit(event, request, response) {
        if (event === 'request' && request && response) {
          try {
            const route = String(request.url || '').split('?')[0];
            if (route === SCRIPT_PATH) {
              serve(request, response);
              return true;
            }
            if (route === REPORT_PATH) {
              receive(request, response);
              return true;
            }
            if (navigation(request)) {
              delete request.headers['accept-encoding'];
              watch(response);
            }
          } catch {}
        }
        return emit.apply(this, arguments);
      };
    }
  }
} catch {}
