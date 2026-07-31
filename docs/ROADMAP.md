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

## M0 — Um cliente entra, anda e vê chunks ✅

**Fechou em 2026-07-31**, com as cinco etapas abaixo cumpridas contra um cliente
Bedrock não modificado. O que vem agora é o [M1](#m1--multiplayer-e-persistência).

**Critério de conclusão:** um cliente Minecraft Bedrock não modificado, na versão-alvo,
encontra o servidor na lista, conecta, spawna, se move com movimentação refletida no
servidor, e vê chunks de um mundo gerado. A sessão sobrevive a 5 minutos sem desconectar.

**Explicitamente fora do M0: nenhum I/O de mundo.** O mundo é gerado em memória e o
disco não é tocado. Ler chunk do disco não prova nada que gerar não prove — o cliente
não sabe a diferença — e antecipar o formato de armazenamento aqui misturaria dois
problemas difíceis num só milestone.

Este é o maior milestone do projeto e é assim de propósito: nada abaixo disso prova que
o protocolo funciona. Internamente ele se divide em cinco etapas, cada uma com seu
próprio critério — eram seis até o proxy de captura ser removido
([ADR-015](DECISIONS.md#adr-015--sem-proxy-de-captura)).

> **Correção de ordem.** A versão anterior deste roadmap colocava o proxy de captura
> como primeira etapa, argumentando que sem ele as demais viram tentativa e erro. Isso
> era circular: um proxy MITM fala RakNet com o cliente **e** com o servidor oficial, e
> termina a criptografia dos dois lados — ou seja, precisa do M0.2 e do M0.3 prontos.
>
> O que quebra o ciclo é que a fase offline viaja em claro. Uma sonda de ping de ~150
> linhas confirma a versão-alvo sem stack nenhuma, e o proxy passa para depois do
> handshake, que é onde ele realmente rende: capturar `StartGame`.

### M0.1 — Sonda de ping offline ✅
- `UnconnectedPing`/`UnconnectedPong`, parser da string de anúncio, e uma sonda que
  pergunta a versão a um servidor real.
- **Fechou quando:** `PROTOCOL_VERSION` e `MINECRAFT_VERSION` deixaram de ser `None`,
  com procedência registrada em [COMPATIBILITY.md](COMPATIBILITY.md#versão-alvo) e
  pongs crus versionados como fixtures.
- Resultado: protocolo `1001`, corroborado por dois servidores independentes — e a
  descoberta de que servidores grandes anunciam número de fachada, o que tornou "uma
  fonte só" insuficiente por evidência, não por precaução.
- **Confirmado no M0.2**, quando um cliente real declarou `1001` em claro no primeiro
  pacote que enviou. Ver [COMPATIBILITY.md](COMPATIBILITY.md#versão-alvo).

### M0.2 — RakNet ✅
- Abertura de conexão, MTU, confiabilidade, ordenação, fragmentação, ACK/NACK, keepalive.
- **Fechou quando:** em 2026-07-30 um cliente Bedrock não modificado encontrou o servidor
  na lista, completou o handshake até `ConnectionRequestAccepted` e enviou o primeiro
  pacote de jogo. Os testes de fragmentação passam com 1 KiB / 64 KiB / 1 MiB, e um
  megabyte atravessa um link simulado com 12% de perda e 25% de reordenação intacto.
- O cliente trava em "Conectando ao servidor externo" porque ninguém responde o login.
  É onde o M0.2 termina de propósito.

### M0.3 — Handshake e criptografia ✅
- `NetworkSettings`, decodificação do `Login`, verificação do token de identidade, ECDH
  P-384, derivação de chave e cifra do stream.
- **Fechou quando:** em 2026-07-30 um cliente Bedrock real completou o handshake, teve
  seu `ClientToServerHandshake` decifrado e validado, decifrou nosso `PlayStatus` de
  login aceito e seguiu para o `ClientCacheStatus` — o primeiro pacote da sequência
  pós-login.
- A dívida que ficou aberta aqui — o servidor não buscava o JWKS e portanto aceitava a
  identidade declarada — foi paga no M0.3b.

### M0.3b — Autenticação online ligada ✅
- Buscar o JWKS do emissor por HTTP, com cache e rotação de chave, e chamar a
  verificação no caminho do servidor.
- **Fechou quando:** em 2026-07-31 o caminho de login passou a ser exercido por um token
  assinado de verdade. O mesmo token é aceito dentro da validade e recusado com
  `PlayStatus` depois dela — mesmos bytes, mesmas chaves, só o relógio mudou — e um
  token com os claims editados é recusado pela assinatura.
- A chave de teste que assina esses tokens está no repositório de propósito: sem uma
  assinatura válida não dá para testar nada além do que é recusado *antes* dela.
- Contra o emissor real: um cliente Bedrock logou e o gamertag veio dos claims
  assinados, não do que o login declarava sobre si.
- O `bedrock-cli` busca as chaves antes de abrir o socket e as renova de hora em hora.
  Uma renovação que falha mantém as chaves anteriores — o que não acontece é o servidor
  passar a aceitar quem não consegue verificar.

### M0.4 — Spawn ✅
- `ResourcePacks*`, `StartGame`, raio de chunk, `LevelChunk`, `PlayStatus` de player
  spawn.
- **Fechou quando:** em 2026-07-30 um jogador saiu da tela de carregamento e apareceu no
  mundo. O cliente enviou `SetLocalPlayerAsInitialized`, depois centenas de
  `PlayerAuthInput` a ~20 por segundo, e a sessão não caiu.
- **Paleta de blocos vazia foi aceita**, confirmando a aposta do
  [ADR-015](DECISIONS.md#adr-015--sem-proxy-de-captura). **Registros vazios não foram** —
  ver [PROTOCOL.md](PROTOCOL.md#registros-vazios-são-recusados).
- O mundo é o vazio: as colunas não têm bloco nenhum porque nomear um bloco exige a
  paleta da versão. Chão sólido é o próximo passo, não parte deste critério.

### M0.5 — Mundo gerado e movimentação ✅
- Gerador flat em memória, seções de chunk imutáveis
  ([ADR-010](DECISIONS.md#adr-010--seções-de-chunk-imutáveis)), `LevelChunk`,
  `NetworkChunkPublisherUpdate`, streaming por raio
  ([ADR-016](DECISIONS.md#adr-016--streaming-de-chunks-por-raio-fixo)), entrada de
  movimentação do jogador.
- **Fechou quando:** em 2026-07-31 um jogador nasceu de pé no chão, caminhou e recebeu
  mundo enquanto andava. O cliente reportou `y=81.62` fixo com a posição horizontal
  avançando — caminhada, não voo — e cada travessia de coluna disparou nove colunas
  novas, com o retorno sobre chão já enviado custando zero.
- O mundo passou a ter dono: `bedrock-blocks` e `bedrock-world` deixaram de ser esboços,
  e a geração saiu de dentro do `bedrock-protocol`, onde estava violando o grafo de
  dependências.
- **O chão sólido revelou um bug que o mundo vazio escondia:** uma posição no fio é
  1,62 acima dos pés, então spawnar na altura da superfície enterrava o jogador. Ver
  [PROTOCOL.md](PROTOCOL.md#uma-posição-no-fio-é-162-acima-dos-pés).

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
