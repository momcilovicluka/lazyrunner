// The demo shop's dev server: serves a page on the port it's given, and plays a shopper - every moment it calls the
// Orders and Billing APIs, so lazyrunner's requests view has traffic. No packages.
import { createServer } from "node:http";

const port = Number(process.argv[2] ?? 47311);
const ORDERS = "http://localhost:47211";
const BILLING = "http://localhost:47212";
const VISITS = [
  ["GET", `${ORDERS}/api/orders?page=1`],
  ["GET", `${ORDERS}/api/orders/1004`],
  ["GET", `${BILLING}/api/invoices/12`],
  ["GET", `${ORDERS}/api/orders?page=2`],
  ["POST", `${ORDERS}/api/orders`],
  ["GET", `${BILLING}/api/invoices/14`],
  ["GET", `${ORDERS}/api/orders/1017`],
  ["GET", `${ORDERS}/api/orders/report`],
  ["GET", `${BILLING}/api/invoices/9`],
  ["GET", `${ORDERS}/api/orders/1099`],
];

createServer((request, response) => {
  response.setHeader("content-type", "text/html");
  response.end("<h1>Demo shop</h1>");
}).listen(port, () => {
  console.log("demo shop dev server");
  console.log(`  ➜  Local:   http://localhost:${port}/`);
});

let next = 0;
setInterval(async () => {
  const [method, url] = VISITS[next++ % VISITS.length];
  try {
    const response = await fetch(url, { method });
    console.log(`shopper: ${method} ${new URL(url).pathname} → ${response.status}`);
  } catch {
    // The APIs are still starting.
  }
}, 900);
