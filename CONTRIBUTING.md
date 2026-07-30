# Contribuindo

Obrigado pelo interesse. Este documento é curto de propósito — as regras que importam
são poucas.

## Antes de escrever código

O projeto está em pré-alpha e a ordem de trabalho é rígida: o
[ROADMAP.md](docs/ROADMAP.md) define o que vem primeiro, e cada milestone tem critério
de conclusão binário. Contribuição fora da ordem provavelmente não será mesclada, ainda
que boa.

**Abra uma issue antes de um PR grande.** Trabalho não alinhado é desperdício do seu
tempo, não do nosso.

Se você quer ajudar e não sabe por onde: as issues marcadas `good first issue` são
autocontidas. Fora delas, o gargalo do projeto está no [M0](docs/ROADMAP.md#m0--um-cliente-entra-anda-e-vê-chunks-reais).

## Regras não negociáveis

Estas existem porque violá-las custa caro depois. Um PR que as viole não é mesclado,
mesmo funcionando.

1. **Respeite o grafo de dependências.** [ARCHITECTURE.md](docs/ARCHITECTURE.md#grafo-de-dependências)
   define quem pode depender de quem. Isso é normativo. Se sua feature exige violar o
   grafo, o que ela exige é um ADR.
2. **Sem `panic!`, `unwrap` ou `expect` em código que processa entrada de rede.** Ver
   [SECURITY.md](SECURITY.md). Erro é `Result` com tipo próprio.
3. **Sem alocação dimensionada por entrada não confiável sem limite explícito.**
4. **Otimização exige medição.** Número antes, número depois, benchmark reproduzível.
   Ver [PERFORMANCE.md](docs/PERFORMANCE.md).
5. **Afirmação sobre o protocolo exige evidência.** Comportamento de protocolo vira teste
   com bytes reais, não comentário citando outro projeto. Ver
   [PROTOCOL.md](docs/PROTOCOL.md#princípios).
6. **Sem código copiado de outros servidores Minecraft.** Ver
   [ADR-007](docs/DECISIONS.md#adr-007--sem-fork-de-projeto-existente). Consultar para
   entender comportamento é esperado; copiar contamina a licença do projeto.
7. **`unsafe` requer justificativa escrita no PR.**

## Decisões de arquitetura

Se o seu PR muda algo que outro código vai passar a depender — modelo de concorrência,
formato em disco, forma da API pública, escolha de dependência estrutural — ele precisa
de um ADR em [DECISIONS.md](docs/DECISIONS.md) **no mesmo PR**.

O formato está lá: contexto, decisão, consequências (**incluindo as ruins**), e o que
faria mudarmos de ideia. Um ADR que só lista vantagens não foi pensado.

## Fluxo

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

Os três passam antes do push. O CI roda o mesmo — falhar nele por formatação é ruído
evitável.

- Branch a partir de `main`.
- Commits no formato [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `perf:`, `chore:`), com o crate como
  escopo quando aplicável: `feat(raknet): fragment reassembly`.
- PR pequeno e com um propósito. PR que faz três coisas é revisado como se fizesse uma —
  mal.
- Descrição do PR responde: o que muda, por quê, e como você sabe que funciona.

## Testes

- Codec: round-trip encode/decode contra fixtures reais.
- Lógica pura: teste unitário no mesmo arquivo.
- Decoders de rede: alvo de fuzz em `fuzz/` (requisito a partir do M1).
- Correção de bug: um teste que falha antes da correção. Sem exceção.

## Idioma

Código, comentários, nomes, mensagens de commit e documentação técnica em **inglês** —
é um projeto open source e a barreira de entrada importa.

Discussão em issues e PRs pode ser em português ou inglês.

## Licença

Ao contribuir, você concorda em licenciar sua contribuição sob a licença MIT do projeto.
