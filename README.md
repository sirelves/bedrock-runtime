# bedrock-runtime

Um servidor Minecraft: Bedrock Edition escrito em Rust.

> **Status: pré-alpha (M0).** Nada aqui roda ainda. O repositório existe primeiro como
> contrato técnico — arquitetura, protocolo e critérios de conclusão estão escritos
> antes do código para que as decisões sejam auditáveis. Veja [DECISIONS.md](docs/DECISIONS.md).

## Por que

Os servidores Bedrock open source existentes rodam em runtimes com garbage collector,
dentro de um loop de tick com orçamento de 50 ms. A tese deste projeto é que um core em
Rust entrega latência previsível — sem as pausas de GC que aparecem como travamento
simultâneo para todo mundo conectado.

**A tese é falseável e tem número:** MSPT p99 abaixo de 25 ms com 50 jogadores, medido
por uma suíte de carga. Esse é o critério do [M2](docs/ROADMAP.md#m2--os-números), e se
ele não fechar, a premissa do projeto está errada.

Não é um fork nem uma reimplementação de nenhum projeto existente. É código novo, com
o protocolo implementado a partir de observação e das referências listadas em
[PROTOCOL.md](docs/PROTOCOL.md).

## Estado atual

| Componente | Estado |
|---|---|
| Transporte RakNet | fase offline decodifica; conexão não iniciada |
| Handshake / criptografia | não iniciado |
| Codec de pacotes | não iniciado |
| Versão-alvo | `1.26.30`, protocolo `1001` ([procedência](docs/COMPATIBILITY.md#versão-alvo)) |
| Mundo e chunks | não iniciado |
| Persistência | fora do M0 ([ADR-011](docs/DECISIONS.md#adr-011--sem-io-de-mundo-no-m0)) |
| API de plugins | apenas contrato, e opcional ([PLUGIN_API.md](docs/PLUGIN_API.md)) |

O critério de conclusão do M0 é objetivo: **um cliente Bedrock não modificado conecta,
se move e recebe chunks de um mundo gerado.** Nenhum I/O de mundo no M0 — ler do disco
não prova nada sobre o protocolo que gerar em memória não prove. Detalhes e milestones
seguintes em [ROADMAP.md](docs/ROADMAP.md).

## Quick start

A versão do Rust está fixada em `rust-toolchain.toml` e é a única testada em CI.

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

**Dentro:** servidor dedicado, uma versão estável do Bedrock por vez, multiplayer,
armazenamento em formato próprio, e os números que validam a tese.

**Fora:** compatibilidade com mundos criados pelo Minecraft vanilla (formato LevelDB),
cliente, proxy entre versões, compatibilidade com Java Edition, Education Edition,
paridade de geração de mundo com a vanilla.

**Opcional:** sistema de plugins. O contrato está escrito; a implementação só faz sentido
depois que o M2 fechar.

## Licença

MIT. Veja [LICENSE](LICENSE).

Não afiliado à Mojang Studios nem à Microsoft. "Minecraft" é marca registrada da
Mojang Studios.
