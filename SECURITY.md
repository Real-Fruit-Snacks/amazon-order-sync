# Security

amazon-order-sync runs on your computer. The server listens on `127.0.0.1` only, refuses any request whose `Host` isn't `127.0.0.1` or `localhost` on its port, and answers only its own dashboard, its own extension (by fixed ID) and local tools without an Origin. Your order history, notes, images and config stay on your disk. The only outside service it calls is the Gemini API, and only when you add a key; for each draft it sends the product's name, brand and listing features, up to three public customer reviews of that product, and up to three of your own posted reviews as style samples. It never sends your order numbers, prices or address. [docs/guide.md](docs/guide.md#security-model) has the details.

The headless browser profile holds a signed-in Amazon session. Treat that folder like a password.

This repository is private. Report a problem by opening an issue, or privately to the owner if it involves someone else's data. I aim to reply within a week. Only the latest release is supported.
