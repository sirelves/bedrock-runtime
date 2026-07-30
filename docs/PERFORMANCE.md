# Desempenho

## A regra

**Nenhuma otimização entra sem medição que a justifique.** Um PR de desempenho precisa
trazer o número antes e o número depois, produzidos pelo mesmo benchmark reproduzível.
"Ficou mais rápido" não é argumento; é opinião com sotaque técnico.

O corolário incomoda mais: até existir a suíte de carga do M2, **as metas abaixo são
hipóteses**, não compromissos. Documentá-las agora serve para definir o que vamos medir
— não para fingir que já sabemos que atingimos.

## O que importa

O orçamento de um tick é 50 ms (20 TPS). A métrica que define a experiência do jogador
não é a média — é a cauda. Um servidor com MSPT médio de 10 ms e p99 de 300 ms trava
visivelmente para todo mundo, ao mesmo tempo, várias vezes por minuto.

Portanto a métrica primária é **p99 de MSPT**, não TPS médio.

## Metas (hipóteses até o M2)

Carga de referência: 50 jogadores, view distance 8.

| Métrica | Meta | Por quê |
|---|---|---|
| MSPT p50 | < 10 ms | folga de 5× no orçamento |
| MSPT p99 | < 25 ms | metade do orçamento na cauda |
| MSPT p99.9 | < 50 ms | **nunca perder um tick** |
| TPS sustentado | 20.0 | qualquer coisa abaixo é degradação visível |
| Memória residente | < 1 GB | previsível e estável |
| Crescimento de memória em 24 h | ~0 | vazamento é bug, não característica |
| Latência de entrada→resposta | < 1 tick | movimento não deve parecer atrasado |

Metas de escala secundárias, a validar: 200 jogadores num único processo mantendo
MSPT p99 < 40 ms.

## Como medimos

### Suíte de carga (pré-requisito, M2)
Clientes sintéticos falando o protocolo real — não mocks. Cenários versionados no
repositório:

- `idle`: N jogadores parados. Mede o custo de existir.
- `walk`: N jogadores em movimento contínuo, forçando streaming de chunk.
- `build`: N jogadores modificando blocos em rajada.
- `join-storm`: N jogadores conectando simultaneamente. É o pior caso do login.

### Instrumentação
- Histograma de MSPT com percentis, não média. Média esconde exatamente o que importa.
- Tempo por fase do tick (rede, entrada, mundo, saída) para localizar o gargalo antes
  de tocar em código.
- Contadores: chunks carregados, entidades, sessões, bytes por segundo, alocações.
- Métricas expostas em formato consumível por Prometheus (M2).

### Ferramentas
- `criterion` para microbenchmarks — codec de pacote, NBT, paleta, compressão.
- `cargo flamegraph` / `perf` para perfilar o loop sob carga.
- `dhat` ou similar para perfil de alocação.

Microbenchmark de codec é o único caso em que aceitamos otimizar sem carga real, porque
o custo é linear no volume de pacotes e o benchmark é fiel.

## Onde o custo provavelmente está

Hipóteses a confirmar, listadas para orientar onde instrumentar primeiro — **não** para
otimizar preventivamente:

1. **Serialização e compressão de chunk.** Grande, frequente, e no caminho de todo
   jogador que se move. Candidato número um a sair do tick para um pool.
2. **Broadcast de movimento.** Cresce com N². Provável primeiro limite de escala.
3. **Remontagem de fragmentos RakNet.** Alocação por fragmento é armadilha conhecida.
4. **Carregamento de chunk do disco.** I/O; precisa estar fora do tick por construção,
   não por otimização.

## Regressão

Quando a suíte de carga existir (M2), ela roda em CI num runner dedicado, e uma
regressão de mais de 10% em MSPT p99 bloqueia o merge.

Antes disso, não há gate de desempenho — e alegar desempenho sem esse gate é alegação
não verificada.

## Anti-metas

Coisas que **não** vamos perseguir:

- Bater um servidor específico em benchmark de marketing.
- Otimizar antes do M2. Correção do protocolo primeiro; um servidor rápido que não deixa
  ninguém entrar tem desempenho zero.
- Micro-otimização de código frio. Se não aparece no perfil, não existe.
- Reduzir uso de memória abaixo da meta às custas de legibilidade.
