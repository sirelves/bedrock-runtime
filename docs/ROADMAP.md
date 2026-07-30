# Roadmap

Cada milestone tem um **critério de conclusão binário** — dá para responder "fechou?"
com sim ou não, sem discussão. Milestone sem critério verificável é lista de desejos.

Não há datas. Há ordem e dependência.

---

## M0 — Um cliente entra, anda e vê chunks reais

**Critério de conclusão:** um cliente Minecraft Bedrock não modificado, na versão-alvo,
encontra o servidor na lista, conecta, spawna, se move com movimentação refletida no
servidor, e vê chunks carregados do disco (não gerados na hora). A sessão sobrevive a
5 minutos sem desconectar.

Este é o maior milestone do projeto e é assim de propósito: nada abaixo disso prova que
o protocolo funciona. Internamente ele se divide em cinco etapas, cada uma com seu
próprio critério.

### M0.1 — Ferramenta de captura
- Proxy MITM entre cliente real e servidor oficial, com dump de pacotes descriptografados.
- **Fecha quando:** um login completo está capturado, descriptografado e salvo como
  fixtures em disco.
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

### M0.5 — Mundo e movimentação
- Leitura de chunk do disco, `LevelChunk`, `NetworkChunkPublisherUpdate`, streaming por
  raio, entrada de movimentação do jogador.
- **Fecha quando:** o critério do M0 acima é satisfeito.

---

## M1 — Multiplayer e persistência

**Critério de conclusão:** dois clientes conectados simultaneamente se veem, veem o
movimento um do outro em tempo real, e quebrar/colocar um bloco é visível para ambos e
sobrevive a um restart do servidor.

Inclui:
- Registro de sessões e broadcast de entidades.
- Quebrar e colocar blocos, com escrita no mundo.
- Persistência de chunk modificado.
- Chat.
- Comandos básicos de operador (`/stop`, `/say`, `/tp`, lista de jogadores).
- Salvamento periódico e shutdown limpo.

---

## M2 — Servidor sustentável

**Critério de conclusão:** o servidor roda 24 horas com 20 jogadores simulados sem
degradação de MSPT e sem crescimento de memória; um restart não perde estado.

Inclui:
- Configuração em arquivo, permissões, allowlist/banlist.
- Métricas expostas (TPS, MSPT, memória, jogadores, chunks carregados).
- Regime de salvamento e recuperação de falha.
- Decisão sobre compatibilidade com mundos vanilla (vira ADR) — se sim, formato em disco
  compatível; se não, formato próprio documentado.
- Suíte de carga automatizada com clientes sintéticos — pré-requisito de
  [PERFORMANCE.md](PERFORMANCE.md).

---

## M3 — Plugins

**Critério de conclusão:** um plugin de terceiro, sem tocar no código do servidor,
registra um comando, reage a um evento de bloco e modifica o mundo — e um plugin que
entra em loop infinito ou estoura memória é contido sem derrubar o servidor.

Inclui:
- Escolha do runtime (vira [ADR-002](DECISIONS.md#adr-002--api-de-plugins-com-runtime-adiado)
  atualizado) — a decisão exige benchmark, não opinião.
- Implementação do contrato de [PLUGIN_API.md](PLUGIN_API.md).
- Sandbox, limites de CPU e memória, modelo de permissões.
- Carregamento, descarregamento e versionamento de plugin.

---

## M4 — Jogo

**Critério de conclusão:** definido quando o M3 fechar. Provavelmente inventário,
entidades vivas, física de blocos, ciclo dia/noite com efeito real.

Escrever critérios detalhados agora seria adivinhação — o que aprendermos nos milestones
anteriores muda o que faz sentido aqui.

---

## Fora do roadmap

Explicitamente não planejado. Se entrar, entra como mudança de escopo com ADR:

- Paridade de geração de mundo com a vanilla (mesma seed, mesmo terreno).
- Suporte a múltiplas versões do Bedrock simultaneamente.
- Compatibilidade com plugins de PocketMine, Nukkit ou similares.
- Interoperabilidade com Java Edition.
- Redstone completa.
