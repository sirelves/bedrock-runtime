# Decisões de arquitetura (ADRs)

Registro do *porquê*. O objetivo é que daqui a seis meses ninguém precise perguntar
"por que fizemos assim?" — e que reverter uma decisão seja um ato consciente, com custo
conhecido, e não um acidente.

Formato: contexto, decisão, consequências (incluindo as ruins), e o que faria mudarmos
de ideia. Um ADR não é apagado; é substituído por outro que o marca como superado.

| # | Decisão | Estado |
|---|---|---|
| [001](#adr-001--core-em-rust) | Core em Rust | Aceita |
| [002](#adr-002--api-de-plugins-com-runtime-adiado) | API de plugins com runtime adiado | Aceita |
| [003](#adr-003--ecs-adiado) | ECS adiado | Aceita |
| [004](#adr-004--protocolo-pinado-em-uma-versão) | Protocolo pinado em uma versão | Aceita |
| [005](#adr-005--sandbox-de-plugins-como-requisito) | Sandbox de plugins como requisito | Aceita |
| [006](#adr-006--monolito-modular) | Monolito modular | Aceita |
| [007](#adr-007--sem-fork-de-projeto-existente) | Sem fork de projeto existente | Aceita |

---

## ADR-001 — Core em Rust

**Contexto.** O loop de tick de um servidor Minecraft tem orçamento de 50 ms. O que
mata a experiência não é a média — é a cauda. Uma pausa de GC de 200 ms no percentil 99
aparece como travamento para todo mundo conectado ao mesmo tempo. Os servidores Bedrock
open source existentes rodam em Java ou PHP e pagam esse custo estruturalmente.

**Decisão.** O core é Rust. Sem GC, latência previsível, e o sistema de tipos usado para
impedir compartilhamento indevido de estado entre threads em tempo de compilação (ver
o modelo de concorrência em [ARCHITECTURE.md](ARCHITECTURE.md#modelo-de-concorrência)).

**Consequências.**
- Positiva: sem pausas de GC; controle explícito de alocação no caminho quente.
- Positiva: o borrow checker torna o modelo de "mundo é de thread única" verificável em
  vez de convencional.
- **Negativa: o pool de contribuidores é muito menor** que o de Java. Para um projeto
  open source, isso é um custo real e permanente, não um detalhe.
- **Negativa: nenhuma biblioteca madura de RakNet em Rust.** Estamos escrevendo transporte
  do zero — o maior risco do M0 é consequência direta desta decisão.
- Negativa: iteração inicial mais lenta que em linguagem dinâmica.

**O que nos faria mudar de ideia.** Se o M0 mostrar que o custo de implementar RakNet do
zero domina o cronograma e a latência não for gargalo real na carga-alvo, a premissa
merece reexame. Métrica que decide: p99 de MSPT em [PERFORMANCE.md](PERFORMANCE.md).

---

## ADR-002 — API de plugins com runtime adiado

**Contexto.** A intenção original era cravar TypeScript como primeira linguagem de
plugins. Escolher o runtime agora (V8 via `deno_core`, QuickJS, ou WASM) significa
escolher sem dado: os três diferem em custo de travessia host↔guest, e esse custo só é
mensurável quando existir um evento de jogo real acontecendo milhares de vezes por tick.

Escolher errado é caro — o runtime define a forma da API, o modelo de sandbox e a
experiência de quem escreve plugin. Trocar depois quebra todo mundo.

**Decisão.** Definir agora o **contrato** da API — superfície de eventos, comandos,
modelo de permissões, forma dos handlers — em [PLUGIN_API.md](PLUGIN_API.md), com
TypeScript como linguagem de referência para descrevê-lo. **Não** escolher o runtime
até o M3, quando houver benchmark.

O contrato é escrito de forma a não pressupor o mecanismo: nada nele exige que o guest
seja JavaScript.

**Consequências.**
- Positiva: a decisão cara é tomada com dado em vez de preferência.
- Positiva: o contrato pode ser revisado por terceiros antes de existir código.
- Negativa: nenhum plugin roda antes do M3.
- Negativa: risco de o contrato descrever algo que o runtime escolhido torne caro. Mitigado
  mantendo a API baseada em eventos em lote, não em callbacks por entidade por tick.

**O que nos faria mudar de ideia.** Um caso de uso concreto que exija plugin antes do M3.

---

## ADR-003 — ECS adiado

**Contexto.** ECS é a resposta padrão para entidades em jogos, e havia intenção de
adotá-lo desde o início. Mas no escopo do M0 e do M1 existem: jogadores. Poucos. Sem
mobs, sem itens no chão, sem projéteis.

Adotar ECS agora significa carregar o custo conceitual e a rigidez de um `World` de ECS
para gerenciar dezenas de entidades homogêneas — e amarrar o resto da arquitetura a
essa escolha antes de saber se ela paga.

**Decisão.** Começar com armazenamento simples (slotmap indexado por id de entidade,
struct por tipo). Reavaliar quando existirem entidades heterogêneas em volume — provável
M4.

Isso é uma **discordância explícita com a intenção inicial do projeto**, registrada aqui
para não parecer omissão: ECS antes de ter entidades é over-engineering.

**Consequências.**
- Positiva: menos complexidade num período em que a complexidade real está no protocolo.
- Positiva: a decisão de ECS será tomada com perfil de carga real.
- **Negativa: migrar para ECS depois é refatoração significativa** — tocará todo o código
  de entidade escrito até lá.
- Mitigação: acesso a entidade fica atrás de uma fronteira estreita em `bedrock-server`,
  de modo que a migração seja localizada.

**O que nos faria mudar de ideia.** Chegar a mais de ~5 tipos de entidade com
comportamentos compostos, ou o perfil mostrar que iterar entidades domina o tick.

---

## ADR-004 — Protocolo pinado em uma versão

**Contexto.** O protocolo do Bedrock quebra a cada versão. Suportar N versões
simultâneas exige uma camada de tradução de pacotes — abstração que custa em todo pacote,
em toda mudança, para sempre. Projetos que tentaram isso gastam a maior parte do esforço
de manutenção nessa camada.

**Decisão.** O servidor suporta **uma** versão estável do Bedrock por vez. A versão-alvo
é uma constante única em `bedrock-protocol`. Não há camada de tradução, nem abstração de
versionamento, nem `if version >= X` espalhado pelo codec.

Quando a versão-alvo mudar, o codec muda junto, num PR, e a versão antiga deixa de ser
suportada. A política está em [COMPATIBILITY.md](COMPATIBILITY.md).

**Consequências.**
- Positiva: o codec permanece legível; sem indireção especulativa.
- Positiva: atualizar de versão é um diff auditável, não uma arqueologia.
- **Negativa: jogadores que ainda não atualizaram o cliente não conseguem entrar.** Não
  há janela de migração. Para um servidor público, isso é um problema operacional real.
- Negativa: se a Mojang lançar uma versão durante um milestone, ela interrompe o trabalho.

**O que nos faria mudar de ideia.** Operação real mostrando que a falta de janela de
migração expulsa jogadores. A resposta então seria suportar N e N-1 — **não** tradução
multi-versão genérica.

---

## ADR-005 — Sandbox de plugins como requisito

**Contexto.** O modelo dominante (Bukkit, PocketMine) dá ao plugin acesso irrestrito ao
processo. Na prática isso significa que instalar um plugin é confiar totalmente no autor:
ele pode ler arquivos, abrir sockets, e travar o servidor com um laço.

**Decisão.** Isolamento de plugin é requisito funcional, não feature opcional. Um plugin
não recebe acesso a filesystem, rede ou processo por padrão; recebe capacidades
declaradas e concedidas. Um plugin que consome CPU ou memória além do limite é contido
sem derrubar o servidor. O critério de conclusão do M3 testa exatamente isso.

**Consequências.**
- Positiva: instalar plugin deixa de ser ato de fé.
- Positiva: falha de plugin vira incidente isolado.
- **Negativa: restringe o que um plugin pode fazer**, e alguns casos legítimos vão exigir
  capacidade explícita — mais atrito para quem escreve plugin.
- Negativa: sandbox custa desempenho na fronteira host↔guest. Quanto, depende do runtime
  ([ADR-002](#adr-002--api-de-plugins-com-runtime-adiado)).

---

## ADR-006 — Monolito modular

**Contexto.** A tentação em projeto novo é separar em serviços "para escalar depois".
Um servidor Minecraft é um loop de tick com estado compartilhado e forte acoplamento
temporal — é o tipo de carga onde distribuir adiciona latência de rede dentro do
orçamento de 50 ms e não resolve nada.

**Decisão.** Um processo, um binário. A modularidade é expressa em crates com grafo de
dependência acíclico e verificado pelo compilador ([ARCHITECTURE.md](ARCHITECTURE.md)).
Os crates existem para forçar disciplina, não para permitir deploy independente.

**Consequências.**
- Positiva: sem serialização, sem rede e sem falha parcial no caminho quente.
- Positiva: depurar é ler uma stack trace.
- Negativa: escala horizontal exige múltiplas instâncias com mundos separados. Aceito —
  é como servidores de Minecraft escalam de fato.
- Negativa: um crash derruba tudo. Mitigado por [ADR-005](#adr-005--sandbox-de-plugins-como-requisito)
  para a origem mais provável de crash (plugin de terceiro).

---

## ADR-007 — Sem fork de projeto existente

**Contexto.** Existem implementações maduras de servidor Bedrock em Java e PHP. Portar
uma delas seria mais rápido no início.

**Decisão.** Código novo. As implementações existentes são consultadas como referência
de comportamento do protocolo ([PROTOCOL.md](PROTOCOL.md#referências)), nunca copiadas.

**Consequências.**
- Positiva: sem contaminação de licença; o projeto pode ser MIT.
- Positiva: a arquitetura é escolhida, não herdada.
- **Negativa: significativamente mais lento até o primeiro cliente conectar.** É o preço
  do M0 ser grande.
- Negativa: erros que aqueles projetos já resolveram serão redescobertos.
- Mitigação: fixtures capturados de tráfego real ([M0.1](ROADMAP.md#m01--ferramenta-de-captura))
  substituem a leitura de código alheio como fonte de verdade.
