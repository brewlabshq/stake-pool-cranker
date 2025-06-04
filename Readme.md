<div align="center">
    
# Stake Pool Cranker

</div>


A Rust-based automation tool for cranking [Stake Pool](https://spl.solana.com/stake-pool/cli) pools on the Solana blockchain using a CLI interface. It is designed to simplify and automate the process of sending cranking transactions using Solana RPC and a designated wallet.

---

## 🧪 Requirements

- Rust (latest stable version)
- Solana CLI (for local key management, optional)

---

## 🔧 Environment Configuration

Create a `.env` file or export the following environment variables:

```env
RPC_URL="https://api.mainnet-beta.solana.com"
FEE_PAYER_PRIVATE_KEY=''  # Your Solana wallet's private key (Base64 or JSON)
STAKE_POOL_ADDRESS=''     # Your Stake Pool address
SLACK_TOKEN=''            # Slack Bot Token
SLACK_CHANNEL_ID=''       # Slack channel id where bot should send the message  
```

<!-- ```` -->

> ⚠️ **Do not expose your `FEE_PAYER_PRIVATE_KEY` in public repos.** Use secrets managers or environment variables for secure deployment.

> **Do not forget to change the message that you want to send on your slack channel. To change the message update line no. 81 in main.rs**

---

## 📁 Project Structure

```
.
├── Cargo.lock              # Cargo dependency lock file
├── Cargo.toml              # Package mainfest
├── Readme.md               # Project documentation
└── src                     
    ├── client.rs           # Client functions to get stake_pool, validator_list etc.
    ├── config.rs           # Loads and validates environment config
    ├── main.rs             # CLI entry point
    └── utils               # Sets the compute budget instructions
        ├── compute_budget.rs
        └── mod.rs
```

---

## 🛠️ Building & Running

### Build Locally

```bash
cargo build --release
```

### Run with CLI

```bash
RPC_URL="https://api.mainnet-beta.solana.com" \
FEE_PAYER_PRIVATE_KEY='<your-private-key>' \
STAKE_POOL_ADDRESS='<your-stake-pool-address>' \
SLACK_TOKEN='<your-slack-token>' \
SLACK_CHANNEL_ID='<your-slack-channel-id>' \
cargo run --release
```
---

## 🤝 Contributing

We welcome contributions from the community! To get started:

1. Fork the repo
2. Create your feature branch (`git checkout -b feature/your-feature`)
3. Commit your changes (`git commit -am 'Add new feature'`)
4. Push to the branch (`git push origin feature/your-feature`)
5. Open a pull request

---

## 📝 License

This project is licensed under the [Apache 2.0 License](LICENSE).

---

## 👨‍💻 Author

Built and maintained by the Brew Labs team.
