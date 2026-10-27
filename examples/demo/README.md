# Demo shop

A small made-up project for trying lazyrunner, and the one the README's recording shows (`docs/demo.tape`): two
ASP.NET APIs and a frontend.

- **Orders** (`services/Orders`, port 47211): lists and loads orders; `/api/orders/report` is slow on purpose.
- **Billing** (`services/Billing`, port 47212): loads invoices; every seventh one fails, on purpose.
- **Shop** (`web/shop`, port 47311): a dev server with no packages that also plays a shopper - it calls the APIs every
  moment, so the requests view (`H`) has something to show.

Needs the .NET SDK (10) and Node.js. Then, in this folder:

```
lazyrunner
```

lazyrunner finds `.lazyrunner.json` here, asks whether to trust it, checks the tools, builds the APIs and starts all
three. Its ports (472xx, 473xx) are below Windows' dynamic range, so Hyper-V and WSL don't reserve them.
