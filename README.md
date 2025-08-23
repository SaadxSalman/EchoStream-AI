# Synapse-360 🔎⚛️

An advanced version of your SynapseSearch project, **Synapse-360** is a cognitive agent for code. It moves beyond keyword-based search to understand the **intent and logic of code**, providing intelligent suggestions, refactoring recommendations, and even automatic bug fixes.

-----

## ✨ Features

  * **Cognitive Code Understanding:** Instead of just matching keywords, the agent understands the semantic meaning and intent behind code snippets.
  * **Intelligent Refactoring:** The **Code Analysis Agent** can identify inefficient or problematic code and suggest optimal refactored solutions.
  * **Automated Bug Fixes:** The **Suggestion Agent** can generate and propose new code to fix identified bugs, streamlining the debugging process.
  * **Chain of Thought (CoT) Prompting:** Uses CoT prompting with a fine-tuned **Gemma-CodeVec** model to reason through complex code logic and provide more accurate results.
  * **Elastic Scalability:** Built to handle massive codebases with **Milvus**, a high-performance vector database that ensures fast and scalable semantic searches.
  * **High-Performance Backend:** The core logic is written in **Rust**, providing a low-latency, real-time code analysis engine.

-----

## ⚙️ Tech Stack

  * **Frontend:** [Next.js](https://nextjs.org/) and [TypeScript](https://www.typescriptlang.org/)
  * **Backend:** Rust
  * **Vector Database:** [Milvus](https://milvus.io/)
  * **Code Embeddings:** Gemma-CodeVec (fine-tuned)

-----

## 🚀 Getting Started

### Prerequisites

  * Rust
  * Node.js and npm (for Next.js)
  * Docker (for Milvus)

### Installation

1.  **Clone the repository:**
    ```bash
    git clone https://github.com/saadsalmanakram/Synapse-360.git
    cd Synapse-360
    ```
2.  **Start Docker containers:**
    ```bash
    docker-compose up -d
    ```
3.  **Build the Rust core:**
    ```bash
    cargo build --release
    ```
4.  **Set up the frontend:**
    ```bash
    cd frontend
    npm install
    ```

### Configuration

Create a `.env` file to configure your API keys or model paths for Gemma-CodeVec.

### Usage

Run the Rust backend and the Next.js frontend to begin your cognitive code searches.

-----

## 🤝 Contributing

We welcome contributions from developers, researchers, and anyone interested in advancing code intelligence. Please read our [CONTRIBUTING.md](https://www.google.com/search?q=https://github.com/saadsalmanakram/Synapse-360/blob/main/CONTRIBUTING.md) for details on submitting pull requests.

-----

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](https://www.google.com/search?q=https://github.com/saadsalmanakram/Synapse-360/blob/main/LICENSE) file for details.
