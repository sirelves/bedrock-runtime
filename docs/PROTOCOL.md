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
  fragmento daí em diante.
- Todos os pacotes offline carregam a constante `MAGIC` de 16 bytes
  (`00 ff ff 00 fe fe fe fe fd fd fd fd 12 34 56 78`).

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
  aceitável para fechar o M0; RTO adaptativo entra no M3.
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
  [SECURITY.md](../SECURITY.md#autenticação).

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

Para o M0 ("entra, anda e vê chunks reais"), o conjunto mínimo é:

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

**Ferramenta prioritária:** um proxy de captura que fique entre um cliente real e o
servidor oficial, com as chaves de sessão em mãos, gerando fixtures automaticamente.
Construir isso na primeira semana do M0 economiza o resto do milestone.
