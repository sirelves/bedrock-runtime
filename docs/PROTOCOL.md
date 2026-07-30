# Protocolo Bedrock — estratégia de implementação

Este documento é **estratégia, não especificação**. O protocolo do Bedrock não é
publicado pela Mojang e muda a cada versão. Escrever uma especificação completa antes
de ter um cliente conectado produziria ficção. O que está aqui é: o que já sabemos, o
que precisa ser confirmado empiricamente, em que ordem atacar, e como registrar o que
descobrirmos.

Conforme cada camada é implementada e validada contra um cliente real, a seção
correspondente sai de "a confirmar" e vira referência com link para o código e para os
testes que provam o comportamento.

## Princípios

1. **Nada entra como verdade sem ter sido observado.** Toda afirmação sobre o protocolo
   precisa de um teste com bytes capturados ou de um cliente real conectando. Documentação
   de terceiros é ponto de partida para a hipótese, nunca a prova.
2. **Bytes capturados são artefato de teste.** Todo comportamento confirmado vira um
   fixture em `crates/bedrock-protocol/tests/fixtures/` com round-trip encode/decode.
3. **Falha de decode nunca derruba o servidor.** Um pacote malformado encerra a sessão
   daquele cliente, e só. Decodificação é código hostil por definição — ver
   [SECURITY.md](../SECURITY.md).
4. **Uma versão por vez.** Sem camada de tradução, sem abstração de versionamento
   especulativa ([ADR-004](DECISIONS.md#adr-004--protocolo-pinado-em-uma-versão)).

## As cinco camadas

O protocolo é uma pilha. Cada camada precisa estar correta antes de a seguinte ser
observável — não dá para pular etapas, e é isso que define a ordem do M0.

```text
5. Pacotes de jogo      Login, StartGame, LevelChunk, MovePlayer, ...
4. Batch                agrupamento + compressão (zlib ou snappy)
3. Criptografia         AES-256, chave derivada de ECDH
2. RakNet               confiabilidade, ordenação, fragmentação sobre UDP
1. UDP                  porta 19132 (IPv4) / 19133 (IPv6)
```

---

## Camada 2 — RakNet

**Estado:** não iniciado. É o item de maior risco do projeto.

RakNet é um protocolo de confiabilidade sobre UDP, originalmente uma biblioteca C++ de
propósito geral para jogos. O Bedrock usa uma variante dele. Não existe implementação
madura em Rust — este é código próprio.

### O que precisa existir

**Fase offline** (antes de haver conexão):
- `UnconnectedPing` / `UnconnectedPong` — é o que faz o servidor aparecer na lista do
  cliente. A resposta do pong carrega o MOTD como uma string de campos separados por `;`
  (nome, versão de protocolo, versão legível, jogadores online, máximo, GUID do servidor,
  entre outros). **A ordem e a quantidade exata de campos precisam ser confirmadas contra
  a versão-alvo** — é comum quebrar aqui e o sintoma é o servidor simplesmente não
  aparecer, sem erro.
- `OpenConnectionRequest1/2` e `OpenConnectionReply1/2` — negociação de MTU. O cliente
  sonda com pacotes de tamanhos decrescentes; o MTU acordado limita o tamanho do
  fragmento daí em diante. **Implementado e confirmado** — ver os três achados abaixo.
- Todos os pacotes offline carregam a constante `MAGIC` de 16 bytes
  (`00 ff ff 00 fe fe fe fe fd fd fd fd 12 34 56 78`).

### Confirmado contra tráfego real (2026-07-30)

Fixtures em `crates/bedrock-raknet/tests/fixtures/`, obtidos com
`cargo run -p bedrock-raknet --example connect`.

**O MTU anunciado inclui os cabeçalhos IP e UDP.** Sondando um servidor com
1492/1400/1200/576 bytes de payload, as respostas foram 1520/1428/1228/604 — exatamente
28 bytes a mais em todos os degraus (20 de IPv4 + 8 de UDP). Tratar o número anunciado
como tamanho de payload coloca todo datagrama cheio 28 bytes acima do limite: funciona
em loopback e fragmenta ou some numa rede real. `payload_limit()` modela isso.

**Os octetos IPv4 vão complementados.** Confirmado por ground truth, não por consenso
entre implementações: o endereço que o servidor reportou para nós só bate com o IP
público real sob a leitura complementada. A armadilha é que a leitura errada produz um
endereço plausível — as duas são complemento uma da outra.

**Existe um cookie de anti-amplificação.** Quando o servidor liga o flag de segurança no
`Reply1`, ele envia um cookie de 4 bytes que o cliente precisa devolver no `Request2`.
Um endereço de origem forjado nunca recebe o cookie, então não passa do request 1. Ler o
flag errado desloca o campo de MTU em vez de falhar o decode — erro silencioso.

**A versão do protocolo RakNet é 11**, e `IncompatibleProtocolVersion` (0x19) devolve a
versão que o servidor fala. Não há motivo para adivinhar: pergunte.

**A escada de MTU é necessária.** Numa execução o degrau de 1492 não teve resposta e o
de 1200 passou; noutra, contra o mesmo servidor, 1492 respondeu. Não é precaução
teórica — o caminho muda.

### Fase conectada — confirmada

Uma conexão RakNet completa foi estabelecida contra um servidor real:
`ConnectionRequest` → `ConnectionRequestAccepted` → `NewIncomingConnection` →
`ConnectedPing`/`ConnectedPong` → `Disconnect`. Isso exercita, contra uma implementação
de verdade, o cabeçalho de datagrama, o número de sequência u24 little-endian, o
comprimento de frame **em bits**, os índices de confiabilidade e ordenação, e a nossa
codificação de ACK.

**O comprimento do frame é em bits, não bytes.** Lido como bytes, o payload sai com um
oitavo do tamanho e todo frame seguinte no datagrama lê do offset errado — vira lixo, não
erro de decode.

**Os slots de endereço não usam o placeholder do RakNet.** O `ConnectionRequestAccepted`
traz 20 slots, e este servidor preenche todos com `0.0.0.0:0` — não com o
`255.255.255.255:0` que o RakNet usa como não-atribuído. As duas convenções significam a
mesma coisa; quem conhecer só uma lê a outra como endereço roteável.

**A quantidade de slots não está no fio.** O RakNet compila 10, servidores Bedrock mandam
20. Decodificar lendo slots até sobrarem só os dois timestamps finais evita a adivinhação,
e a resposta espelha a contagem recebida.

**Ranges de ACK ficam ranges.** Um único record pode reivindicar 16 milhões de números de
sequência; expandir isso numa lista é negação de serviço com um pacote.

**Fase conectada** (o trabalho real):
- *Datagrama* com número de sequência próprio, carregando um ou mais *frames*.
- Confiabilidades: unreliable, unreliable sequenced, reliable, reliable ordered,
  reliable sequenced (e as variantes com ACK receipt). O jogo usa principalmente
  **reliable ordered no canal 0**.
- Fragmentação: payload maior que o MTU vira N fragmentos com `split_id`, `split_index`
  e `split_count`; a remontagem é responsabilidade do receptor.
- ACK / NACK com ranges de sequência, retransmissão por RTO estimado a partir do RTT.
- `ConnectedPing` / `ConnectedPong` para keepalive e medição de RTT.
- `Disconnect`.

### Riscos concretos

- **Remontagem de fragmentos é superfície de ataque.** Um cliente pode anunciar
  `split_count` enorme e nunca enviar os fragmentos. Limite de memória por sessão e
  timeout de remontagem são requisito, não otimização.
- **Retransmissão mal calibrada mata o desempenho antes do jogo existir.** RTO fixo é
  aceitável para fechar o M0; RTO adaptativo entra no M2, com os números na mão.
- **A ordem de janela de ordenação é por canal.** Misturar canais é fonte comum de
  travamento silencioso.

### Critério de conclusão da camada

Um cliente Bedrock real vê o servidor na lista de mundos e completa a abertura de
conexão até `ConnectionRequestAccepted`. Testes de fragmentação com payloads de 1 KB,
64 KB e 1 MB fazem round-trip.

---

## Camada 3 — Criptografia e autenticação

**Estado:** não iniciado. Segundo maior risco.

O fluxo, na ordem em que acontece:

1. Cliente envia `RequestNetworkSettings`; servidor responde `NetworkSettings` — é aqui
   que o **algoritmo de compressão é negociado** (zlib ou snappy, dependendo da versão)
   e o limiar de compressão é definido. Este passo acontece **antes** de qualquer
   criptografia. Errar isso faz todo o resto parecer corrompido.
2. Cliente envia `Login`, contendo:
   - uma **cadeia de JWTs** (`chain`) com a identidade do jogador, assinada em ES384.
     A cadeia é validada até a chave pública raw da Mojang; a última entrada carrega o
     `identityPublicKey` do cliente, o XUID e o nome de exibição.
   - um JWT separado com os dados do cliente (skin, dispositivo, idioma), assinado pela
     mesma chave.
3. Servidor gera um par de chaves efêmero **ECDH P-384**, faz o acordo com a chave
   pública do cliente, e deriva a chave de sessão a partir do segredo compartilhado
   combinado com um `salt` aleatório (SHA-256).
4. Servidor envia `ServerToClientHandshake` — um JWT contendo sua chave pública e o
   salt. **A partir do byte seguinte, o stream está cifrado nas duas direções.**
5. Cliente responde `ClientToServerHandshake` (já cifrado). Se isso chegar e decodificar,
   a criptografia está correta.

### A confirmar empiricamente

- **Modo da cifra.** Historicamente AES-256-CFB8 com IV derivado do segredo; versões
  mais recentes usam GCM. Isso muda entre versões e **precisa ser confirmado contra a
  versão-alvo antes de escrever o código** — é uma linha de diferença e um dia de
  depuração.
- **Contador de pacotes.** Há um checksum/contador por pacote que entra na cifra. A
  fórmula exata é a segunda causa mais comum de "descriptografa mas vira lixo".
- Se a validação da cadeia da Mojang será obrigatória por padrão (modo online) — ver
  [SECURITY.md](../SECURITY.md#2-handshake-e-autenticação).

### Critério de conclusão da camada

`ClientToServerHandshake` chega, descriptografa e valida. Teste de round-trip da cifra
com vetores fixos. Cadeia JWT inválida é rejeitada com erro tipado, não com panic.

---

## Camada 4 — Batch e compressão

**Estado:** não iniciado. Baixo risco, mas fácil de errar sutilmente.

Pacotes de jogo não vão sozinhos na rede. Vários são concatenados (cada um prefixado
pelo seu tamanho em varint) num batch, o batch é comprimido, cifrado, e só então entregue
ao RakNet como um payload com prefixo `0xFE`.

Pontos de atenção:
- O algoritmo é o negociado em `NetworkSettings` — e existe um limiar abaixo do qual o
  batch vai **sem compressão**, com um byte indicando isso. Ignorar o limiar funciona
  no login e quebra depois.
- zlib aqui é **raw deflate** (sem cabeçalho zlib). Usar a variante errada dá erro de
  "invalid header" no cliente.
- Limite de tamanho do batch descomprimido é obrigatório (bomba de descompressão).

---

## Camada 5 — Pacotes de jogo

**Estado:** não iniciado.

Para o M0 ("entra, anda e vê chunks"), o conjunto mínimo é:

| Pacote | Direção | Papel |
|---|---|---|
| `RequestNetworkSettings` / `NetworkSettings` | C→S / S→C | negocia compressão |
| `Login` | C→S | identidade e chave pública |
| `ServerToClientHandshake` / `ClientToServerHandshake` | S→C / C→S | inicia criptografia |
| `PlayStatus` | S→C | sinaliza login aceito e, depois, "player spawn" |
| `ResourcePacksInfo` / `ResourcePackStack` / `ResourcePackClientResponse` | ambos | handshake de pacotes; no M0 respondemos "nenhum" |
| `StartGame` | S→C | o pacote mais pesado do login — dimensão, seed, gamerules, paleta de itens, posição de spawn |
| `LevelChunk` | S→C | dados de chunk |
| `NetworkChunkPublisherUpdate` | S→C | define o raio ao redor do qual o cliente aceita chunks |
| `SetLocalPlayerAsInitialized` | C→S | o cliente terminou de carregar |
| `MovePlayer` / `PlayerAuthInput` | C→S | movimentação |
| `SetTime`, `SetDifficulty`, `BiomeDefinitionList`, `AvailableActorIdentifiers` | S→C | estado inicial esperado pelo cliente |
| `Disconnect` | S→C | encerramento com motivo legível |

Notas de risco:
- **`StartGame` é onde a maioria das tentativas trava.** Ele é grande, tem muitos campos
  cujo significado é obscuro, e a ordem importa. Um campo a mais ou a menos e o cliente
  desconecta sem mensagem útil.
- **A paleta de blocos não pode ser escrita à mão.** É um dump NBT de todos os estados
  de bloco da versão, com centenas de entradas. Precisa ser extraída e versionada como
  artefato de dados. Ver [COMPATIBILITY.md](COMPATIBILITY.md).
- `PlayerAuthInput` substitui `MovePlayer` nas versões que usam movimentação
  autoritativa do servidor. Qual dos dois vale depende da flag enviada em `StartGame` —
  no M0 escolhemos o modo mais simples e registramos qual.

---

## Referências

Usadas como fonte de hipótese, sempre validadas contra tráfego real. Nenhum código é
copiado — as licenças e as linguagens são incompatíveis com este projeto de qualquer
forma.

| Fonte | Utilidade | Cuidado |
|---|---|---|
| [CloudburstMC/Protocol](https://github.com/CloudburstMC/Protocol) (Java) | Mapa mais completo de pacotes por versão | Acompanha versões próprias; conferir a versão-alvo |
| [PocketMine-MP](https://github.com/pmmp/PocketMine-MP) (PHP) | Semântica de jogo e ordem do login | Faz escolhas próprias que não são o vanilla |
| [PrismarineJS/bedrock-protocol](https://github.com/PrismarineJS/bedrock-protocol) (JS) | Fluxo de autenticação e criptografia legível | Cobertura de pacotes de jogo incompleta |
| [wiki.vg](https://wiki.vg/) | Contexto geral | Cobertura de Bedrock é parcial e envelhece |
| Captura própria (proxy MITM entre cliente e servidor oficial) | **A única fonte que prova** | Requer chaves de sessão; montar cedo no M0 |

**Ferramenta prioritária, mas não a primeira.** Um proxy de captura entre um cliente
real e o servidor oficial, gerando fixtures automaticamente, é o que torna a Camada 5
viável — `StartGame` é grande demais para ser adivinhado. Só que ele fala RakNet dos dois
lados e termina a criptografia nas duas pontas, então depende das camadas 2 e 3: é o
[M0.4](ROADMAP.md#m04--proxy-de-captura), não o começo.

O que dá para fazer antes de qualquer coisa é a fase offline, que viaja em claro. Foi
assim que a versão-alvo foi confirmada — ver [COMPATIBILITY.md](COMPATIBILITY.md#versão-alvo).
