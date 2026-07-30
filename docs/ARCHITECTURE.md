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
              ┌──────────┼──────────┐
              │          │          │
       bedrock-world  bedrock-protocol
              │          │       │
        bedrock-nbt ─────┘       │
                                 │
                     ┌───────────┴───────────┐
              bedrock-crypto           bedrock-raknet
```

Regras:

- `bedrock-raknet` **não conhece Minecraft.** É transporte genérico. Se um tipo de pacote
  do jogo aparecer nesse crate, o design está errado.
- `bedrock-protocol` **não conhece estado de jogo.** Codifica e decodifica bytes; não
  sabe o que é um jogador nem um mundo. É puro e testável sem I/O.
- `bedrock-world` **não conhece rede.** Manipula chunks e blocos; quem serializa isso
  para o cliente é `bedrock-server` usando `bedrock-protocol`.
- `bedrock-server` é o único crate que conhece todos os outros. É onde vive o loop de
  tick e a orquestração.
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
risco do M0. Ver [PROTOCOL.md](PROTOCOL.md#raknet).

### `bedrock-crypto`
Cadeia de autenticação (JWT/JWS da identidade do Xbox Live), acordo de chaves ECDH,
derivação da chave de sessão, cifra do stream, e compressão/descompressão de batches.

Isolado num crate próprio porque é a superfície onde erro vira vulnerabilidade, e
porque queremos poder auditá-la sem ler o resto do servidor.

### `bedrock-nbt`
NBT nas variantes que o Bedrock usa: little-endian (arquivos) e "network little-endian"
(varints, protocolo). Serialização e desserialização, com e sem `serde`.

Crate folha, sem dependências internas. Deve ser fuzzável isoladamente.

### `bedrock-protocol`
Definição dos pacotes do jogo e seu codec. Um tipo por pacote, um enum `Packet` que
os agrega, e o par encode/decode. Inclui os tipos primitivos do protocolo (varint,
`BlockPos`, `Vec3`, UUID, `ActorRuntimeId`).

A versão-alvo do protocolo é uma constante única neste crate. Não há camada de tradução
entre versões — ver [ADR-004](DECISIONS.md#adr-004--protocolo-pinado-em-uma-versão).

### `bedrock-world`
Representação de mundo: chunk, subchunk, paleta de blocos, estados de bloco, entidades
posicionadas em chunk. Carregamento e escrita do formato em disco. Geração de terreno
(gerador flat no M0; qualquer coisa além disso é pós-M1).

### `bedrock-server`
O loop de tick, o registro de sessões de jogador, o roteamento de pacotes para handlers,
o broadcast de mudanças de estado, comandos e — quando existir — o host da API de plugins.

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
  pronto, nunca uma referência ao estado do mundo.

Consequência prática: se uma feature exige que dois threads mutem o mundo, ela precisa
de um ADR antes do código.

## Fluxo de um pacote

```text
UDP ──► bedrock-raknet ──► descriptografa+descomprime (bedrock-crypto)
                                      │
                                      ▼
                            decodifica (bedrock-protocol)
                                      │
                                      ▼
                              [fila de entrada]
                                      │
        ═══════════ fronteira: aqui começa o loop de tick ═══════════
                                      │
                                      ▼
                      handler em bedrock-server muta o mundo
                                      │
                                      ▼
                               [fila de saída]
                                      │
        ═══════════ fronteira: aqui termina o loop de tick ══════════
                                      │
                                      ▼
              codifica ──► comprime+cifra ──► bedrock-raknet ──► UDP
```

O tick nunca bloqueia em I/O. Se um handler precisa de dados do disco, ele registra a
intenção, o I/O acontece fora, e o resultado entra pela fila de entrada num tick futuro.

## O que ainda não está decidido

Honestidade sobre os limites deste documento — estas partes serão fechadas quando houver
código rodando, e cada uma vira um ADR:

- **Armazenamento de entidades.** Slotmap simples no M0. ECS foi avaliado e adiado
  ([ADR-003](DECISIONS.md#adr-003--ecs-adiado)).
- **Runtime de plugins.** O contrato existe; o mecanismo não ([ADR-002](DECISIONS.md#adr-002--api-de-plugins-com-runtime-adiado)).
- **Formato de persistência do mundo.** Depende de a compatibilidade com mundos vanilla
  ser ou não requisito — ver [ROADMAP.md](ROADMAP.md) M2.
- **Estratégia de view distance e streaming de chunks.** Depende dos números do primeiro
  benchmark ([PERFORMANCE.md](PERFORMANCE.md)).
