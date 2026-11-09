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
