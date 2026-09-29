# capsule-emit-mesh-plugin

A [mesh-llm](https://github.com/Mesh-LLM/mesh-llm) plugin that keeps a signed,
hash-chained record of every exchange a node serves or asks for, on that
node's own disk, and adds an **Evidence** page to the mesh-llm console. The
page shows the peers this node exchanged with, each exchange, and whether the
node's log still verifies.

Records follow the Agent Action Capsule format: JCS-canonical JSON, Ed25519
COSE_Sign1 signatures, and a checkpointed Merkle log.

The plugin source arrives through a pull request, so it gets CI and review
before it lands. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
