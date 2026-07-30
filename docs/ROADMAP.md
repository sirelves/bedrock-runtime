# Roadmap

Cada milestone tem um **critério de conclusão binário** — dá para responder "fechou?"
com sim ou não, sem discussão. Milestone sem critério verificável é lista de desejos.

Não há datas. Há ordem e dependência.

## O que o projeto está tentando provar

Que um servidor Bedrock em Rust entrega latência previsível — sem as pausas que um
runtime com GC impõe ao loop de tick ([ADR-001](DECISIONS.md#adr-001--core-em-rust)).

Isso ordena tudo o que vem abaixo: **protocolo primeiro, depois números, o resto depois
disso.** Compatibilidade com mundos vanilla e sistema de plugins não são requisitos do
projeto; são coisas que podem ou não acontecer sem que a tese falhe.

---

## M0 — Um cliente entra, anda e vê chunks

**Critério de conclusão:** um cliente Minecraft Bedrock não modificado, na versão-alvo,
encontra o servidor na lista, conecta, spawna, se move com movimentação refletida no
servidor, e vê chunks de um mundo gerado. A sessão sobrevive a 5 minutos sem desconectar.

**Explicitamente fora do M0: nenhum I/O de mundo.** O mundo é gerado em memória e o
disco não é tocado. Ler chunk do disco não prova nada que gerar não prove — o cliente
não sabe a diferença — e antecipar o formato de armazenamento aqui misturaria dois
problemas difíceis num só milestone.

Este é o maior milestone do projeto e é assim de propósito: nada abaixo disso prova que
o protocolo funciona. Internamente ele se divide em cinco etapas, cada uma com seu
próprio critério.

### M0.1 — Ferramenta de captura
- Proxy MITM entre cliente real e servidor oficial, com dump de pacotes descriptografados.
- **Fecha quando:** um login completo está capturado, descriptografado e salvo como
  fixtures em disco, e a versão-alvo e o número de protocolo estão preenchidos em
  `version.rs` e em [COMPATIBILITY.md](COMPATIBILITY.md#versão-alvo).
- *Ordem primeiro de propósito.* Sem isso, as etapas seguintes são tentativa e erro.

### M0.2 — RakNet
- Ping/pong offline, abertura de conexão, MTU, confiabilidade, ordenação, fragmentação,
  ACK/NACK, keepalive.
- **Fecha quando:** o servidor aparece na lista de mundos do cliente e a conexão chega
  a `ConnectionRequestAccepted`; testes de fragmentação com 1 KB / 64 KB / 1 MB passam.

### M0.3 — Handshake e criptografia
- `NetworkSettings`, validação da cadeia JWT, ECDH, derivação de chave, cifra do stream,
  compressão de batch.
- **Fecha quando:** `ClientToServerHandshake` chega, descriptografa e valida; um
  `PlayStatus` de login aceito chega ao cliente.

### M0.4 — Spawn
- `ResourcePacks*`, `StartGame`, `BiomeDefinitionList`, paleta de blocos como artefato
  de dados, `PlayStatus` de player spawn.
- **Fecha quando:** o cliente sai da tela de carregamento e mostra o jogador num mundo
  (ainda que vazio), sem desconectar por 60 segundos.

### M0.5 — Mundo gerado e movimentação
- Gerador flat em memória, seções de chunk imutáveis
  ([ADR-010](DECISIONS.md#adr-010--seções-de-chunk-imutáveis)), `LevelChunk`,
  `NetworkChunkPublisherUpdate`, streaming por raio, entrada de movimentação do jogador.
- **Fecha quando:** o critério do M0 acima é satisfeito.

---

## M1 — Multiplayer e persistência

**Critério de conclusão:** dois clientes conectados simultaneamente se veem, veem o
movimento um do outro em tempo real, e quebrar/colocar um bloco é visível para ambos e
sobrevive a um restart do servidor.

Inclui:
- Registro de sessões e broadcast de entidades.
- Quebrar e colocar blocos.
- **Formato de armazenamento próprio**, desenhado agora que se sabe como o chunk é
  representado em memória. Vira ADR.
- Chat.
- Comandos básicos de operador (`/stop`, `/say`, `/tp`, lista de jogadores).
- Salvamento periódico e shutdown limpo.

---

## M2 — Os números

**O milestone que valida ou derruba a tese do projeto.**

**Critério de conclusão:** com a carga de referência de [PERFORMANCE.md](PERFORMANCE.md)
— 50 jogadores, view distance 8 — o servidor sustenta 20 TPS com **MSPT p99 abaixo de
25 ms e p99.9 abaixo de 50 ms**, e roda 24 horas sem crescimento de memória.

Inclui:
- Suíte de carga com clientes sintéticos falando o protocolo real. Pré-requisito de
  todo o resto do milestone.
- Instrumentação: histograma de MSPT com percentis, tempo por fase do tick, contadores.
- Gate de regressão em CI: mais de 10% de piora no p99 bloqueia merge.
- Otimização — e só aqui. Antes disso, otimizar é adivinhação
  ([PERFORMANCE.md](PERFORMANCE.md#a-regra)).
- Configuração em arquivo, allowlist/banlist, salvamento resiliente.

Se os números não fecharem, [ADR-001](DECISIONS.md#adr-001--core-em-rust) precisa ser
reexaminado em vez de defendido. É para isso que o critério é numérico.

---

## M3 — Plugins *(opcional)*

**Não é requisito do projeto.** Só faz sentido se o M2 fechar e houver alguém querendo
escrever plugin. Registrado aqui porque o contrato já existe e seria desperdício perdê-lo.

**Critério de conclusão:** um plugin de terceiro, sem tocar no código do servidor,
registra um comando, reage a um evento de bloco e modifica o mundo — e um plugin que
entra em loop infinito ou estoura memória é contido sem derrubar o servidor.

Inclui a escolha do runtime — que exige benchmark, não opinião
([ADR-002](DECISIONS.md#adr-002--api-de-plugins-com-runtime-adiado)) — a implementação
de [PLUGIN_API.md](PLUGIN_API.md), sandbox, limites e modelo de permissões.

---

## M4 — Jogo

**Critério de conclusão:** definido quando o M2 fechar. Provavelmente inventário,
entidades vivas, física de blocos.

Escrever critérios detalhados agora seria adivinhação — o que aprendermos nos milestones
anteriores muda o que faz sentido aqui.

---

## Fora do roadmap

Explicitamente não planejado. Se entrar, entra como mudança de escopo com ADR:

- **Compatibilidade com mundos criados pelo Minecraft vanilla** (LevelDB). Ver
  [COMPATIBILITY.md](COMPATIBILITY.md#mundos).
- Paridade de geração de mundo com a vanilla (mesma seed, mesmo terreno).
- Suporte a múltiplas versões do Bedrock simultaneamente.
- Compatibilidade com plugins de PocketMine, Nukkit ou similares.
- Interoperabilidade com Java Edition.
- Redstone completa.
