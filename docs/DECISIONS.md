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
| [008](#adr-008--vocabulário-de-blocos-como-crate-folha) | Vocabulário de blocos como crate folha | Aceita |
| [009](#adr-009--crypto-só-com-primitivas) | `crypto` só com primitivas | Aceita |
| [010](#adr-010--seções-de-chunk-imutáveis) | Seções de chunk imutáveis | Aceita |
| [011](#adr-011--sem-io-de-mundo-no-m0) | Sem I/O de mundo no M0 | Aceita |
| [012](#adr-012--raknet-sans-io) | RakNet sans-io | Aceita |
| [013](#adr-013--ring-para-criptografia) | `ring` para criptografia | Aceita |

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
- Mitigação: fixtures capturados de tráfego real ([M0.4](ROADMAP.md#m04--proxy-de-captura))
  substituem a leitura de código alheio como fonte de verdade.

---

## ADR-008 — Vocabulário de blocos como crate folha

**Contexto.** Nomear um bloco é necessário nos dois lados do sistema: `bedrock-world`
precisa armazenar qual bloco está em cada posição, e `bedrock-protocol` precisa
codificar isso para o cliente. Mas o grafo de dependências proíbe que um dependa do
outro ([ARCHITECTURE.md](ARCHITECTURE.md#grafo-de-dependências)), e o tipo não tinha
dono — o que na prática significa que o primeiro dos dois a ser escrito ia arrastar o
outro junto.

Há uma sutileza que resolve o desenho: **identidade de bloco e runtime id são coisas
diferentes.** O runtime id é um número atribuído por versão do protocolo, válido só
naquele contexto de rede. O armazenamento nunca deveria vê-lo.

**Decisão.** Um crate folha `bedrock-blocks` com a identidade do bloco — nome com
namespace e propriedades de estado. `bedrock-world` e `bedrock-protocol` dependem dele.
A **paleta de runtime ids fica em `bedrock-protocol`**, porque é conceito de rede.

**Consequências.**
- Positiva: os dois lados compartilham vocabulário sem se acoplarem.
- Positiva: o formato em disco fica imune a mudança de versão de protocolo — o disco
  guarda nomes, não ids.
- Negativa: mais um crate. Aceito: a alternativa é o acoplamento que o grafo existe
  para impedir.
- Negativa: converter nome→runtime id na serialização de chunk custa uma indireção.
  Se aparecer no perfil, resolve-se com cache — não com acoplamento.

---

## ADR-009 — `crypto` só com primitivas

**Contexto.** `bedrock-crypto` tinha acumulado três responsabilidades: validação da
cadeia JWT do Xbox Live, criptografia de sessão, e compressão de batch.

Duas delas não pertencem ali. Compressão não é criptografia. E a cadeia de login conhece
Minecraft — XUID, dados de skin, o formato do pacote `Login` — o que quebrava a regra
de que crates abaixo do `server` não conhecem o jogo. O resultado é que o único crate
que precisa ser auditável isoladamente era o que mais misturava camadas.

**Decisão.** `bedrock-crypto` fica com ECDH, derivação de chave e cifra do stream.
Cadeia de login e compressão/batching vão para `bedrock-protocol`, que é onde o formato
de fio mora.

**Consequências.**
- Positiva: auditoria de segurança criptográfica passa a ter escopo pequeno e real.
- Positiva: `crypto` deixa de conhecer Minecraft e volta a respeitar o grafo.
- Negativa: `bedrock-protocol` fica maior e passa a conter lógica sensível (validação de
  assinatura). Mitigado mantendo-a num módulo próprio, com as mesmas regras de revisão.

---

## ADR-010 — Seções de chunk imutáveis

**Contexto.** [PERFORMANCE.md](PERFORMANCE.md#onde-o-custo-provavelmente-está) aponta
serialização e compressão de chunk como a primeira candidata a sair do loop de tick —
é grande, frequente, e está no caminho de todo jogador que se move. Mas
[ARCHITECTURE.md](ARCHITECTURE.md#modelo-de-concorrência) proíbe entregar referência ao
estado vivo do mundo para outra thread.

Serializar um chunk **é** ler o mundo. Com seções mutáveis, essas duas regras se
contradizem e a otimização mais importante do projeto fica impossível por construção.
Descobrir isso depois de escrever `bedrock-world` seria reescrevê-lo.

**Decisão.** Seções de chunk são imutáveis e compartilhadas por `Arc`. Modificar um
bloco produz uma seção nova (copy-on-write); o chunk troca o ponteiro. Serialização
recebe um `Arc` de dado congelado e pode ir para qualquer thread.

**Consequências.**
- Positiva: serialização e compressão saem do tick sem nenhum lock.
- Positiva: snapshot para salvamento em disco é grátis — é clonar um `Arc`.
- **Negativa: modificar um bloco copia a seção inteira.** Para construção em rajada isso
  é caro. Mitigação prevista: agrupar mutações do mesmo tick numa única cópia por seção.
  Se o perfil mostrar que não basta, a resposta é uma seção mutável *dentro do tick* que
  congela ao final — não abrir mão da imutabilidade na fronteira.
- Negativa: mais alocação. Medir no M2 antes de reagir.

---

## ADR-011 — Sem I/O de mundo no M0

**Contexto.** O critério original do M0 dizia "chunks reais carregados do disco". Isso
contradizia [COMPATIBILITY.md](COMPATIBILITY.md), que deixava o formato de armazenamento
em aberto — não dá para ler do disco sem escolher o formato. E as duas saídas eram ruins:
implementar o LevelDB do Bedrock no M0 (trabalho do tamanho do RakNet, com dependência
C++) ou inventar um formato provisório que seria descartado no M1.

O ponto que resolve: **o cliente não sabe de onde veio o chunk.** Ler do disco não prova
nada sobre o protocolo que gerar em memória não prove.

**Decisão.** O M0 não toca o disco. Mundo gerado em memória. Persistência inteira vai
para o M1, quando a representação em memória já estiver estável e o formato puder ser
desenhado em cima dela em vez de adivinhado.

**Consequências.**
- Positiva: o M0 tem um problema difícil (protocolo) em vez de dois.
- Positiva: o formato em disco será desenhado sabendo como o chunk é representado.
- Negativa: nada é persistido até o M1 — reiniciar perde tudo. Irrelevante enquanto o
  objetivo é um cliente conectar.

---

## Nota sobre prioridade

Com o objetivo do projeto sendo demonstrar viabilidade técnica em Rust
([ROADMAP.md](ROADMAP.md#o-que-o-projeto-está-tentando-provar)), dois itens deixam de
ser requisito e passam a ser opcionais, sem que isso invalide nenhum ADR:

- **Sistema de plugins** ([ADR-002](#adr-002--api-de-plugins-com-runtime-adiado),
  [ADR-005](#adr-005--sandbox-de-plugins-como-requisito)) — o contrato continua válido
  e as decisões continuam de pé; o M3 é opcional.
- **Compatibilidade com mundos vanilla** — fora de escopo, não adiada.

O que **não** é opcional é o M2: é ele que valida ou derruba
[ADR-001](#adr-001--core-em-rust), que é a premissa do projeto inteiro.

---

## ADR-012 — RakNet sans-io

**Contexto.** A máquina de estado de uma sessão RakNet precisa de tempo (timeouts,
retransmissão, keepalive) e de rede (datagramas entrando e saindo). O caminho óbvio é
ela possuir um socket e um relógio: `Session::run().await`.

Isso custa caro em três frentes. Testar retransmissão passa a exigir dormir de verdade —
um teste de backoff de 2 segundos leva 2 segundos, e a suíte fica lenta o bastante para
ninguém rodar. Testar perda e reordenação passa a exigir uma rede falsa ou uma real.
E o crate passa a arrastar um runtime assíncrono para dentro de uma camada que é, no
fundo, um autômato sobre bytes.

**Decisão.** `Session` não toca socket e não lê relógio. Datagramas e o instante atual
entram por parâmetro; datagramas e payloads saem por retorno. Quem tem o socket é a
camada acima.

```text
receive(bytes, now) -> payloads
send(payload, now)
tick(now)
poll_transmit() -> Option<datagrama>
```

**Consequências.**
- Positiva: backoff, expiração e morte por retry são testados sem dormir. A suíte inteira
  do crate roda em centésimos de segundo.
- Positiva: perda, duplicação e reordenação se testam movendo `Vec<u8>` entre duas
  sessões em memória — sem rede, sem flakiness.
- Positiva: o crate continua sem dependências. Tokio entra em `bedrock-server`, onde o
  I/O de fato mora ([ARCHITECTURE.md](ARCHITECTURE.md#modelo-de-concorrência)).
- **Negativa: a camada acima precisa fazer o loop** — chamar `tick`, drenar
  `poll_transmit`, decidir quando acordar. Não é ergonômico como um `.await`.
- Negativa: `now` aparece em quase toda assinatura pública. Verboso, e é o preço direto
  da testabilidade.

**O que nos faria mudar de ideia.** Se o driver acabar tão complexo que o bug migre para
ele, a fronteira está no lugar errado. Sinal a observar: `bedrock-server` precisando
replicar estado que já existe dentro de `Session`.

---

## ADR-013 — `ring` para criptografia

**Contexto.** Até aqui o projeto tinha **zero dependências externas** em oito crates.
Isso não foi acidente, mas também não é sustentável: o M0.3 precisa verificar
assinaturas RS256, fazer acordo de chaves ECDH P-384 e cifrar um stream. Escrever
qualquer uma dessas à mão é exatamente o que o [SECURITY.md](../SECURITY.md) proíbe.

A escolha inicial foi RustCrypto, pelo argumento de que o `ring` não implementa
AES-CFB8 — e o modo da cifra do Bedrock ainda não foi confirmado. Aí os números
apareceram.

**Medição.**

| conjunto | crates transitivos |
|---|---|
| `ring` + `serde_json` + `base64` | **15** |
| RustCrypto: `rsa` + `p384` + `sha2` | **49** |

E o `rsa` do RustCrypto carrega a [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)
(Marvin attack), **aberta e sem correção**. Ela afeta operações com chave privada, e nós
só verificamos com chave pública — mas o `cargo-deny` do nosso CI bloqueia merge, então
usá-la exigiria silenciar uma advisory de segurança. Isso envelhece mal.

**Decisão.** `ring` para verificação RSA e para o ECDH P-384. Se a cifra do Bedrock for
CFB8, entram `aes` e `cfb8` do RustCrypto — que são crates pequenos, não os 49 acima (o
número é dominado pelo `num-bigint-dig` do `rsa` e pela pilha `elliptic-curve` do `p384`).

**Consequências.**
- Positiva: um terço das dependências, e nenhuma advisory para silenciar.
- Positiva: `ring` é assembly derivado do BoringSSL, auditado, e verificação de
  assinatura é precisamente o que ele faz melhor.
- Negativa: traz toolchain C na build. Os alvos do CI (x86-64 e aarch64) são suportados.
- Negativa: se a cifra for CFB8, o projeto passa a ter duas famílias de cripto. Aceito —
  são preocupações independentes, e a alternativa era escolher a família inteira apostando
  num modo de cifra que ainda não medimos.
- **Negativa: acabou a marca de zero dependências.** O `CONTRIBUTING.md` exige
  justificativa por dependência nova; esta é a justificativa.

**Correção registrada.** Eu havia recomendado RustCrypto neste projeto **antes de medir**,
com base no argumento do CFB8. Medindo, o argumento não sobrevive: o escape do CFB8 custa
poucos crates, e a diferença de footprint e de advisory é grande. A recomendação anterior
estava errada.

**O que nos faria mudar de ideia.** O `ring` ficar sem manutenção — nesse caso o
`aws-lc-rs` é substituto quase drop-in.
