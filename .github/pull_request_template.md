## O que muda

<!-- Uma frase. Se precisar de tres, provavelmente sao tres PRs. -->

## Por que

<!-- Link para a issue ou para o milestone em docs/ROADMAP.md. -->

## Como sei que funciona

<!-- Teste, fixture, captura, benchmark. "Testei localmente" nao e evidencia. -->

## Checklist

- [ ] `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all` passa
- [ ] Respeita o grafo de dependencias de `docs/ARCHITECTURE.md`
- [ ] Sem `panic!`/`unwrap`/`expect` em caminho que processa entrada de rede
- [ ] Sem alocacao dimensionada por entrada nao confiavel sem limite explicito
- [ ] Afirmacao sobre o protocolo tem teste com bytes reais
- [ ] Se muda desempenho: numero antes e depois, com benchmark reproduzivel
- [ ] Se e decisao estrutural: ADR em `docs/DECISIONS.md` neste mesmo PR
- [ ] Se corrige bug: existe teste que falha sem a correcao
