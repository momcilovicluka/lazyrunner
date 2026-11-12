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
