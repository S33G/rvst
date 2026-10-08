import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import assert from 'node:assert/strict';
const html = readFileSync(new URL('../assets/index.html', import.meta.url), 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
function element() {
  return { value: '', checked: false, children: [], textContent: '', appendChild(child) { this.children.push(child); }, replaceChildren(...children) { this.children = children; }, get lastChild() { return this.children.at(-1); } };
}
function batch(events = [], value = 1) {
  return { events, last_sequence: events.at(-1)?.sequence || 0, instance: { plugin_name: 'Fixture', plugin_id: 'org.test', pid: 1, id: 'test' }, metrics: { callbacks: 10, max_callback_ns: 1000, mean_callback_ns: 500, p95_upper_ns: 1023, p99_upper_ns: 1023, deadline_misses: 0, ui_frames: 1 }, dropped_events: 0, snapshot: { live: { parameters: [{ id: 42, name: 'Gain', value, unit: 'x', min: 0, max: 2, pending: null }] } } };
}
const event = sequence => ({ sequence, unix_ms: 0, level: 'debug', target: 'plugin', message: `event ${sequence}`, data: { sequence } });
async function fixture(initial) {
  const elements = new Map();
  const get = id => { if (!elements.has(id)) elements.set(id, element()); return elements.get(id); };
  get('level').value = 'all';
  const responses = [initial], timers = [], echoed = [];
  let now = 10000;
  class Clock extends Date { static now() { return now; } }
  const context = vm.createContext({ location: { hash: '#test-token' }, Date: Clock, document: { getElementById: get, createElement: element, createDocumentFragment: element }, fetch: async () => ({ ok: true, json: async () => responses.shift() }), setTimeout: callback => timers.push(callback), console: Object.fromEntries(['debug', 'info', 'warn', 'error'].map(level => [level, (...args) => echoed.push({ level, args })])) });
  vm.runInContext(script, context);
  await new Promise(resolve => setImmediate(resolve));
  return { get, echoed, context, setTime(value) { now = value; }, async poll(response) { responses.push(response); await timers.shift()(); } };
}
test('console echo is opt-in, follows event levels, and never replays already seen events', async () => {
  const f = await fixture(batch([event(1)]));
  assert.equal(f.echoed.length, 0);
  f.get('console-echo').checked = true; f.get('console-echo').onchange();
  await f.poll(batch([event(1), { ...event(2), level: 'error' }], 1.5));
  assert.equal(f.echoed.length, 1);
  assert.equal(f.echoed[0].level, 'error');
  assert.match(f.echoed[0].args[0], /event 2/);
  assert.equal(f.get('parameter-values').children[0].children[0].children[1].textContent, '1.5 x');
  await f.poll(batch([event(2)]));
  assert.equal(f.echoed.length, 1);
});
test('console output is rate limited while browser history remains bounded', async () => {
  const f = await fixture(batch());
  f.get('console-echo').checked = true; f.get('console-echo').onchange();
  await f.poll(batch(Array.from({ length: 1500 }, (_, i) => event(i + 1))));
  assert.equal(f.echoed.length, 50);
  assert.equal(vm.runInContext('events.length', f.context), 1000);
  f.setTime(11001);
  await f.poll(batch([event(1501)]));
  assert.equal(f.echoed.length, 52);
  assert.match(f.echoed[50].args[0], /1450 console events suppressed/);
  f.get('console-echo').checked = false; f.get('console-echo').onchange();
  await f.poll(batch([event(1502)]));
  assert.equal(f.echoed.length, 52);
});
