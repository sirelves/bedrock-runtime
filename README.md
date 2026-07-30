# bedrock-runtime

Um servidor Minecraft: Bedrock Edition escrito em Rust.

> **Status: pré-alpha (M0).** Nada aqui roda ainda. O repositório existe primeiro como
> contrato técnico — arquitetura, protocolo e critérios de conclusão estão escritos
> antes do código para que as decisões sejam auditáveis. Veja [DECISIONS.md](docs/DECISIONS.md).

## Por que

Os servidores Bedrock existentes forçam uma escolha ruim: o servidor oficial da Mojang
é fechado e não extensível, e as alternativas open source pagam o custo de runtimes
com garbage collector em um loop de tick de 50 ms. `bedrock-runtime` aposta que um core
em Rust entrega latência previsível (sem pausas de GC) e uma superfície de plugins segura
por construção.

Não é um fork nem uma reimplementação de nenhum projeto existente. É código novo, com
o protocolo implementado a partir de observação e das referências listadas em
[PROTOCOL.md](docs/PROTOCOL.md).

## Estado atual

| Componente | Estado |
|---|---|
| Transporte RakNet | não iniciado |
| Handshake / criptografia | não iniciado |
| Codec de pacotes | não iniciado |
| Mundo e chunks | não iniciado |
| API de plugins | apenas contrato ([PLUGIN_API.md](docs/PLUGIN_API.md)) |

O critério de conclusão do M0 é objetivo: **um cliente Bedrock não modificado conecta,
se move e recebe chunks reais de um mundo carregado do disco.** Detalhes e milestones
seguintes em [ROADMAP.md](docs/ROADMAP.md).

## Quick start

Requer Rust 1.90+ (a versão exata está em `rust-toolchain.toml`).

```bash
git clone https://github.com/sirelves/bedrock-runtime
cd bedrock-runtime
cargo build
cargo test
cargo run -p bedrock-cli -- --help
```

Enquanto o M0 não fechar, `bedrock-cli` sobe apenas os subsistemas já implementados e
sai. Não tente conectar um cliente ainda.

## Documentação

| Documento | O que responde |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Como o sistema é dividido e quem pode depender de quem |
| [ROADMAP.md](docs/ROADMAP.md) | Milestones e critérios objetivos de conclusão |
| [PROTOCOL.md](docs/PROTOCOL.md) | Estratégia para implementar o protocolo Bedrock |
| [PLUGIN_API.md](docs/PLUGIN_API.md) | Contrato da API de plugins |
| [COMPATIBILITY.md](docs/COMPATIBILITY.md) | Versões do Bedrock suportadas e política de atualização |
| [PERFORMANCE.md](docs/PERFORMANCE.md) | Metas de TPS/MSPT, profiling e benchmarks |
| [DECISIONS.md](docs/DECISIONS.md) | ADRs — por que cada decisão foi tomada |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Como contribuir |
| [SECURITY.md](SECURITY.md) | Modelo de ameaças e reporte de vulnerabilidades |

## Escopo

**Dentro:** servidor dedicado, uma versão estável do Bedrock por vez, mundos em disco,
plugins, multiplayer.

**Fora:** cliente, proxy entre versões, compatibilidade com Java Edition, suporte a
Education Edition, geração de mundo idêntica à vanilla (paridade de seed).

## Licença

MIT. Veja [LICENSE](LICENSE).

Não afiliado à Mojang Studios nem à Microsoft. "Minecraft" é marca registrada da
Mojang Studios.
