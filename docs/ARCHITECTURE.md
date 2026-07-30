# Arquitetura

Este documento define os limites entre componentes e quem pode depender de quem.
Ele é normativo: um PR que viole o grafo de dependências abaixo é rejeitado, mesmo
que compile.

## Forma geral

**Monolito modular.** Um único processo, um único binário, vários crates com fronteiras
explícitas. Não há microsserviços, não há IPC interno, não há mensageria. O motivo está
em [ADR-006](DECISIONS.md#adr-006--monolito-modular).

Os crates existem para forçar disciplina de dependência — o compilador recusa um
import que atravesse camada — e não para permitir deploy independente.

## Grafo de dependências

Setas apontam para o que o crate pode usar. O grafo é acíclico e a checagem é o próprio
`cargo build`.

```text
                          bedrock-cli
                               │
                          bedrock-server
        ┌──────────┬───────────┼───────────┬──────────┐
        │          │           │           │          │
 bedrock-raknet  bedrock-crypto│    bedrock-world      │
                               │           │           │
                       bedrock-protocol    │           │
                               │           │           │
                        ┌──────┴──────┬────┴───────────┘
                        │             │
                  bedrock-nbt   bedrock-blocks
```

`bedrock-server` é o único crate que conhece todos os outros. `raknet`, `crypto`,
`protocol` e `world` são **irmãos**: nenhum deles enxerga os outros. Quem orquestra a
pilha (descriptografar → descomprimir → decodificar) é o `server`.

Regras:

- `bedrock-raknet` **não conhece Minecraft.** É transporte genérico. Se um tipo de pacote
  do jogo aparecer nesse crate, o design está errado.
- `bedrock-crypto` **não conhece Minecraft.** Só primitivas de sessão. Se um campo do
  `Login` aparecer ali, ele foi para o crate errado.
- `bedrock-protocol` **não conhece estado de jogo.** Codifica e decodifica bytes; não
  sabe o que é um jogador nem um mundo. É puro e testável sem I/O.
- `bedrock-world` **não conhece rede.** Manipula chunks e blocos; quem serializa isso
  para o cliente é `bedrock-server` usando `bedrock-protocol`.
- `bedrock-nbt` e `bedrock-blocks` são folhas: sem dependências internas, sem I/O.
- Nenhum crate abaixo de `bedrock-server` faz logging de nível `info` ou acima. Camadas
  de baixo retornam erros; a decisão de reportar é de quem chamou.

## Responsabilidades por crate

### `bedrock-raknet`
Transporte confiável sobre UDP. Descoberta offline (ping/pong), abertura de conexão,
MTU discovery, ordenação, fragmentação e remontagem, ACK/NACK, retransmissão, detecção
de timeout.

Expõe: um `Listener` que produz `Connection`s, e em cada `Connection` um par
`send(bytes, reliability)` / `recv() -> bytes`. Nada além disso.

Não existe crate maduro de RakNet em Rust — este é código próprio e é o item de maior
risco do M0. Ver [PROTOCOL.md](PROTOCOL.md#camada-2--raknet).

### `bedrock-crypto`
Acordo de chaves ECDH P-384, derivação da chave de sessão, cifra do stream.
Primitivas, e só.

Isolado num crate próprio porque é a superfície onde erro vira vulnerabilidade, e
porque queremos poder auditá-la sem ler o resto do servidor — o que exige que ela não
misture camadas ([ADR-009](DECISIONS.md#adr-009--crypto-só-com-primitivas)).

### `bedrock-nbt`
NBT nas variantes que o Bedrock usa: little-endian (arquivos) e "network little-endian"
(varints, protocolo). Serialização e desserialização.

Crate folha. Deve ser fuzzável isoladamente.

### `bedrock-blocks`
A identidade de um bloco: nome com namespace e propriedades de estado. Nada além disso.

Existe porque `world` e `protocol` precisam nomear blocos e nenhum dos dois pode
depender do outro ([ADR-008](DECISIONS.md#adr-008--vocabulário-de-blocos-como-crate-folha)).

### `bedrock-protocol`
O formato de fio: cadeia de login, batching, compressão, e a definição e o codec dos
pacotes do jogo. Inclui os tipos primitivos do protocolo (varint, `BlockPos`, `Vec3`,
UUID, `ActorRuntimeId`) e a **paleta de runtime ids** — que é um identificador de rede
por versão, não um conceito de armazenamento.

A versão-alvo do protocolo é uma constante única neste crate. Não há camada de tradução
entre versões — ver [ADR-004](DECISIONS.md#adr-004--protocolo-pinado-em-uma-versão).

### `bedrock-world`
Chunks, subchunks, geração de terreno. **Seções de chunk são imutáveis e compartilhadas
(`Arc`), mutadas por copy-on-write** ([ADR-010](DECISIONS.md#adr-010--seções-de-chunk-imutáveis)).

No M0 o mundo é gerado em memória e não toca o disco. Persistência entra no M1.

### `bedrock-server`
O loop de tick, o registro de sessões de jogador, o roteamento de pacotes para handlers,
o broadcast de mudanças de estado, comandos e — se existir — o host da API de plugins.

### `bedrock-cli`
Binário. Parsing de argumentos, carregamento de configuração, inicialização de logging,
sinais de shutdown. Lógica de jogo aqui é bug.

## Modelo de concorrência

Uma decisão de arquitetura, não de implementação, portanto normativa:

- **O estado do mundo é de thread única.** Um loop de tick, sem locks no caminho quente.
  Compartilhar mundo entre threads com `Arc<RwLock<_>>` é a forma mais rápida de
  transformar um problema de jogo num problema de contenção.
- **I/O é assíncrono e vive fora do tick.** Sockets, disco e qualquer chamada externa
  rodam em Tokio. A fronteira entre o mundo assíncrono e o tick são duas filas:
  entrada (pacotes decodificados) e saída (pacotes a enviar).
- **Trabalho pesado e paralelizável é explicitamente despachado.** Geração de chunk,
  compressão e serialização podem ir para um pool. O que volta para o tick é resultado
  pronto — ou um `Arc` de dado imutável, nunca uma referência ao estado vivo do mundo.

Consequência prática: se uma feature exige que dois threads mutem o mundo, ela precisa
de um ADR antes do código.

### Filas e backpressure

As duas filas são **limitadas**. Fila ilimitada alimentada pela rede é um DoS de uma
linha, e `SECURITY.md` promete conter exaustão de recursos — então a política precisa
estar aqui e não na cabeça de quem implementar:

| Fila | Limite | Quando enche |
|---|---|---|
| Entrada, por sessão | pequeno (ordem de dezenas de pacotes) | encerra a sessão |
| Entrada, global | proporcional ao máximo de jogadores | recusa **novas** conexões; sessões existentes seguem |
| Saída, por sessão | pequeno | encerra a sessão |

Encerrar em vez de descartar é deliberado: o canal principal do jogo é *reliable
ordered*, então descartar um pacote não é uma degradação — é corromper o stream. Um
cliente que produz mais rápido do que o servidor consome está com defeito ou atacando,
e nos dois casos derrubar a sessão é a resposta correta. Ficar sem folga global é
condição de sobrecarga do servidor, não culpa de uma sessão específica; por isso a
resposta ali é parar de aceitar gente nova.

## Fluxo de um pacote

```text
UDP ──► bedrock-raknet ──► descriptografa (bedrock-crypto)
                                      │
                                      ▼
                   descomprime + decodifica (bedrock-protocol)
                                      │
                                      ▼
                          [fila de entrada, limitada]
                                      │
        ═══════════ fronteira: aqui começa o loop de tick ═══════════
                                      │
                                      ▼
                      handler em bedrock-server muta o mundo
                                      │
                                      ▼
                          [fila de saída, limitada]
                                      │
        ═══════════ fronteira: aqui termina o loop de tick ══════════
                                      │
                                      ▼
        codifica + comprime ──► cifra ──► bedrock-raknet ──► UDP
```

Cada seta entre crates irmãos passa pelo `server` — os crates não se chamam entre si.

O tick nunca bloqueia em I/O. Se um handler precisa de dados do disco, ele registra a
intenção, o I/O acontece fora, e o resultado entra pela fila de entrada num tick futuro.

## O que ainda não está decidido

Honestidade sobre os limites deste documento — estas partes serão fechadas quando houver
código rodando, e cada uma vira um ADR:

- **Armazenamento de entidades.** Slotmap simples no M0. ECS foi avaliado e adiado
  ([ADR-003](DECISIONS.md#adr-003--ecs-adiado)).
- **Runtime de plugins.** O contrato existe; o mecanismo não, e plugins não são requisito
  do projeto ([ADR-002](DECISIONS.md#adr-002--api-de-plugins-com-runtime-adiado)).
- **Formato de persistência do mundo.** Formato próprio, desenhado no M1. Compatibilidade
  com mundos vanilla está fora de escopo ([COMPATIBILITY.md](COMPATIBILITY.md#mundos)).
- **Estratégia de view distance e streaming de chunks.** Depende dos números do primeiro
  benchmark ([PERFORMANCE.md](PERFORMANCE.md)).
