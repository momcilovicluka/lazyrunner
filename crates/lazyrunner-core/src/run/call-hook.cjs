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
  const find = (host, at, route) => {
    load();
    const fault = faults.find(
      (each) => targets(each.target, host, at) && route.toLowerCase().startsWith(String(each.path || '').toLowerCase()),
    );
    return fault && Math.random() * 100 < fault.share ? fault : undefined;
  };
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
        return through((opts, done) => {
          setTimeout(() => {
            try {
              done(null, real(opts));
            } catch (error) {
              done(error);
            }
          }, fault.ms);
        });
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
        await new Promise((done) => setTimeout(done, fault.ms));
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
