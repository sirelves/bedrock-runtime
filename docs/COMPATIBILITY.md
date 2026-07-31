# Compatibilidade

## Política

**Uma versão estável do Bedrock por vez.** Sem janela de migração, sem camada de
tradução, sem suporte a versões antigas. A justificativa e os custos aceitos estão em
[ADR-004](DECISIONS.md#adr-004--protocolo-pinado-em-uma-versão).

Consequência direta e importante: **quando a versão-alvo é atualizada, clientes que não
atualizaram deixam de conectar.** Não é um bug e não haverá flag para contornar.

## Versão-alvo

A versão-alvo é uma constante única em `crates/bedrock-protocol/src/version.rs`. Esse
arquivo é a fonte de verdade — a tabela abaixo é derivada dele, não o contrário.

| Campo | Valor | Confiança |
|---|---|---|
| Versão do Minecraft Bedrock | `1.26.30` | apenas exibição; nome diverge entre fontes |
| Número do protocolo | `1001` | **confirmada** — declarada por um cliente real |
| Algoritmo de compressão negociado | *a confirmar no M0.3* | — |
| Modo da cifra | *a confirmar no M0.3* | — |

**Como esses números foram obtidos.** Em 2026-07-30, quatro servidores públicos foram
sondados com `cargo run -p bedrock-raknet --example ping`. Dois deles — Lifeboat e
NetherGames, operadores sem relação entre si, rodando software diferente — anunciaram
protocolo `1001` para a versão `1.26.30`. Os pongs crus estão versionados em
`crates/bedrock-raknet/tests/fixtures/` e um teste os prende.

**O que isso não é.** Autoridade. Os outros dois servidores sondados anunciaram
protocolo `121` e `1` — números de fachada na frente de proxies multi-versão, junto com
contadores como `20001/100001` jogadores. Um servidor de terceiro anuncia o que o
operador configurou. O que dá peso ao `1001` é a concordância entre dois independentes,
não o número em si.

**Confirmado por ground truth em 2026-07-30.** Um cliente Minecraft atualizado conectou
ao nosso servidor e declarou `1001` no `RequestNetworkSettings` — um `int32` em claro,
**antes de qualquer criptografia**, logo depois do handshake RakNet.

Isso vale mais que as três fontes anteriores (dois servidores públicos e a wiki) juntas:
elas dizem o que *servidores* anunciam, e o que importa é o que o *cliente* fala.

O caminho até ali passou por um cliente desatualizado que declarou `975` — o protocolo da
release 26.23. Não era o `1001` estar errado; era o cliente estar atrás. E foi barato
descobrir: uma conexão, oito bytes, nenhuma linha de criptografia.

Fixtures das duas capturas e um teste que falha se a constante divergir do que o cliente
declarou: `crates/bedrock-protocol/tests/first_contact.rs`.

**O nome da versão é só exibição.** Os servidores públicos anunciam `1.26.30`, a wiki
chama a release de `26.35`. O cliente compara o número, não o nome, então nada depende de
resolver isso.

## Histórico de versões suportadas

Preenchido a cada atualização de versão-alvo.

| Versão do Bedrock | Protocolo | Release do servidor | Período |
|---|---|---|---|
| — | — | — | — |

## Como uma atualização de versão acontece

Quando a Mojang lança uma versão nova:

1. Capturar um login com o cliente novo usando um cliente real conectando ao servidor.
2. Fazer o diff dos fixtures contra a versão anterior — é isso que revela o que mudou.
3. Extrair novamente os artefatos de dados (paleta de blocos, definições de bioma,
   identificadores de entidade).
4. Atualizar `version.rs` e o codec afetado, num único PR.
5. Atualizar a tabela acima e cortar um release.

Só há suporte a uma versão por vez, então não há branch de manutenção da anterior.

## Artefatos de dados

Certas coisas não podem ser escritas à mão e são extraídas do cliente/servidor oficial
para cada versão:

| Artefato | O que é | Origem |
|---|---|---|
| Paleta de blocos | NBT com todos os estados de bloco da versão | dump da versão-alvo |
| Runtime ids | índice de um bloco na lista canônica ordenada | `scripts/block-runtime-id.py` |
| Definições de bioma | NBT enviado em `BiomeDefinitionList` | dump da versão-alvo |
| Identificadores de entidade | lista enviada em `AvailableActorIdentifiers` | dump da versão-alvo |
| Paleta de itens | mapeamento nome→id runtime | dump da versão-alvo |

**Runtime id não é adivinhável.** É a posição do estado de bloco na lista canônica da
versão, e `minecraft:air` é `13094`, não `0`. Assumir que ar é zero preenche o mundo com
o que quer que ordene primeiro. O `scripts/block-runtime-id.py` deriva um id da lista
publicada; um mundo flat precisa de dois de dezesseis mil, então eles são constantes no
código em vez de asset — o script é o que os mantém reproduzíveis em vez de mágicos.

Os artefatos maiores ficam versionados em `assets/<versão>/` e são carregados em runtime. **Não** são gerados
por código nem embutidos no binário: são dados da Mojang, com versão própria, e misturá-los
ao código é o que torna atualização de versão dolorosa em outros projetos.

## Plataformas do servidor

| Plataforma | Estado |
|---|---|
| Linux x86-64 | alvo primário |
| Linux aarch64 | suportado, CI |
| macOS (Apple Silicon) | desenvolvimento; sem garantia de produção |
| Windows | não testado, sem suporte |

A versão do Rust está fixada em `rust-toolchain.toml` e é a única testada em CI. Não há
compromisso de MSRV: prometer uma faixa de versões que ninguém verifica é pior do que
não prometer nada.

## Clientes

| Cliente | Estado |
|---|---|
| Bedrock em Windows / console / mobile | alvo |
| Bedrock Preview / Beta | não suportado |
| Education Edition | fora de escopo |
| Java Edition | fora de escopo, permanentemente |

## Mundos

**Compatibilidade com mundos criados pelo Minecraft vanilla está fora de escopo.**

Isso significa implementar o formato LevelDB do Bedrock, com suas particularidades de
chave e de serialização por subchunk, mais uma dependência C++ — trabalho da ordem de
grandeza do RakNet, em troca de um benefício (importar mundo existente) que não ajuda em
nada a provar a tese do projeto ([ROADMAP.md](ROADMAP.md#o-que-o-projeto-está-tentando-provar)).

O armazenamento é **formato próprio**, desenhado no [M1](ROADMAP.md#m1--multiplayer-e-persistência)
em cima da representação de chunk em memória que o M0 já terá estabilizado. No M0 não
existe armazenamento nenhum: o mundo é gerado em memória
([ADR-011](DECISIONS.md#adr-011--sem-io-de-mundo-no-m0)).

Um mundo produzido por este servidor não abre no Minecraft, e um mundo do Minecraft não
abre aqui. Isso é decisão, não limitação temporária.
