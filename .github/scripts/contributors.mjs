// Writes contributors.json - the About box's list - from GitHub: who wrote code (merged pull requests) and who
// reported bugs and ideas (issues, by the forms' labels). No commit counts: they change with every push, and the list
// would with them. .github/contributors-overrides.json pins people (with a role or name), hides them, and sets who
// counts as core. `updated` changes only when the list does.
//
// In the contributors workflow; locally: GITHUB_TOKEN=<token> node .github/scripts/contributors.mjs

import { readFileSync, writeFileSync, existsSync } from "node:fs";

const REPO = process.env.GITHUB_REPOSITORY ?? "momcilovicluka/lazyrunner";
const TOKEN = process.env.GITHUB_TOKEN;
const OUT = "contributors.json";
const PAGE = 100;

const overrides = JSON.parse(readFileSync(".github/contributors-overrides.json", "utf8"));
const pinned = overrides.people ?? {};
const hidden = new Set(overrides.hide ?? []);
const core = { prs: 1, issues: 3, ...overrides.core };

async function api(path) {
  const headers = {
    accept: "application/vnd.github+json",
    "x-github-api-version": "2022-11-28",
    "user-agent": "lazyrunner-contributors",
  };
  if (TOKEN) headers.authorization = `Bearer ${TOKEN}`;
  const response = await fetch(`https://api.github.com${path}`, { headers });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json();
}

async function all(path) {
  const items = [];
  for (let page = 1; ; page++) {
    const batch = await api(`${path}${path.includes("?") ? "&" : "?"}per_page=${PAGE}&page=${page}`);
    items.push(...batch);
    if (batch.length < PAGE) return items;
  }
}

const isBot = (user) => !user || user.type === "Bot" || user.login.endsWith("[bot]");

const people = new Map();
function person(login) {
  if (!people.has(login)) people.set(login, { login, prs: 0, bugs: 0, ideas: 0, issues: 0 });
  return people.get(login);
}

for (const pull of await all(`/repos/${REPO}/pulls?state=closed`)) {
  if (pull.merged_at && !isBot(pull.user)) person(pull.user.login).prs++;
}
for (const issue of await all(`/repos/${REPO}/issues?state=all`)) {
  if (issue.pull_request || isBot(issue.user)) continue;
  const reporter = person(issue.user.login);
  const labels = issue.labels.map((label) => label.name);
  reporter.issues++;
  if (labels.includes("bug")) reporter.bugs++;
  else if (labels.includes("enhancement")) reporter.ideas++;
}

const order = Object.keys(pinned);
for (const login of order) person(login);
const listed = [...people.values()]
  .filter((one) => !hidden.has(one.login))
  .filter((one) => one.login in pinned || one.prs >= core.prs || one.issues >= core.issues)
  .sort((a, b) => {
    const at = (one) => (order.includes(one.login) ? order.indexOf(one.login) : order.length);
    return at(a) - at(b) || b.prs - a.prs || b.issues - a.issues || a.login.localeCompare(b.login);
  });

for (const one of listed) {
  const own = pinned[one.login] ?? {};
  one.name = own.name ?? (await api(`/users/${one.login}`)).name ?? one.login;
  one.role = own.role ?? "";
}

const shaped = listed.map(({ login, name, role, prs, bugs, ideas, issues }) => ({
  login, name, role, prs, bugs, ideas, issues,
}));
const before = existsSync(OUT) ? JSON.parse(readFileSync(OUT, "utf8")) : {};
const same = JSON.stringify(before.people) === JSON.stringify(shaped);
const updated = same && before.updated ? before.updated : new Date().toISOString().slice(0, 10);
writeFileSync(OUT, JSON.stringify({ updated, people: shaped }, null, 2) + "\n");
console.log(`${shaped.length} people${same ? " (unchanged)" : ""}: ${shaped.map((one) => one.login).join(", ")}`);
