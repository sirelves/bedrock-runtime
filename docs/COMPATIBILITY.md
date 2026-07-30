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

| Campo | Valor |
|---|---|
| Versão do Minecraft Bedrock | *a definir no M0.1* |
| Número do protocolo | *a definir no M0.1* |
| Algoritmo de compressão negociado | *a confirmar no M0.3* |
| Modo da cifra | *a confirmar no M0.3* |

Os valores estão em branco de propósito. Preenchê-los com números plausíveis antes de
um cliente real confirmar seria inventar dado — e um número de protocolo errado é a
causa mais comum de "o servidor não aparece na lista", sem erro visível.

Eles serão preenchidos na etapa [M0.1](ROADMAP.md#m01--ferramenta-de-captura), quando a
captura de um login real confirmar cada um.

## Histórico de versões suportadas

Preenchido a cada atualização de versão-alvo.

| Versão do Bedrock | Protocolo | Release do servidor | Período |
|---|---|---|---|
| — | — | — | — |

## Como uma atualização de versão acontece

Quando a Mojang lança uma versão nova:

1. Capturar um login com o cliente novo usando a ferramenta do M0.1.
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
| Definições de bioma | NBT enviado em `BiomeDefinitionList` | dump da versão-alvo |
| Identificadores de entidade | lista enviada em `AvailableActorIdentifiers` | dump da versão-alvo |
| Paleta de itens | mapeamento nome→id runtime | dump da versão-alvo |

Ficam versionados em `assets/<versão>/` e são carregados em runtime. **Não** são gerados
por código nem embutidos no binário: são dados da Mojang, com versão própria, e misturá-los
ao código é o que torna atualização de versão dolorosa em outros projetos.

## Plataformas do servidor

| Plataforma | Estado |
|---|---|
| Linux x86-64 | alvo primário |
| Linux aarch64 | suportado, CI |
| macOS (Apple Silicon) | desenvolvimento; sem garantia de produção |
| Windows | não testado, sem suporte |

MSRV (versão mínima do Rust) está em `rust-toolchain.toml`. Aumentar a MSRV é mudança
menor de versão, não maior.

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
