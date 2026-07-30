# Segurança

## Reportar uma vulnerabilidade

**Não abra issue pública para vulnerabilidade.**

Use [GitHub Security Advisories](https://github.com/sirelves/bedrock-runtime/security/advisories/new)
para reporte privado.

Compromisso de resposta: confirmação de recebimento em até 72 horas, avaliação inicial
em até 7 dias. Divulgação coordenada — publicamos o advisory junto com a correção, com
crédito a quem reportou, salvo pedido em contrário.

Enquanto o projeto está em pré-alpha (M0–M2), **não há release em produção para
corrigir**. Reportes ainda são bem-vindos e viram issue de segurança rastreada.

## Modelo de ameaças

Um servidor de jogo é, por definição, um processo que executa parsing de entrada
arbitrária vinda da internet, sem autenticação prévia, em UDP. O modelo abaixo assume
isso.

### Agentes

| Agente | Capacidade | Confiança |
|---|---|---|
| Atacante não autenticado | envia UDP arbitrário para a porta | **nenhuma** |
| Jogador autenticado | envia pacotes de jogo válidos | baixa — autenticado ≠ honesto |
| Autor de plugin | roda código no servidor | limitada — ver [ADR-005](docs/DECISIONS.md#adr-005--sandbox-de-plugins-como-requisito) |
| Operador do servidor | controla config e binário | total |

### Superfícies, por ordem de risco

**1. Decodificação de pacotes (`bedrock-raknet`, `bedrock-protocol`, `bedrock-nbt`)**

A superfície mais exposta do projeto: código que processa bytes de quem ainda não provou
ser ninguém. Requisitos, não recomendações:

- Nenhum `unwrap`, `expect` ou `panic!` em caminho de decode. Erro é `Result`.
- Nenhuma alocação dimensionada por campo do atacante sem limite superior explícito.
  Um `Vec::with_capacity(n)` onde `n` vem da rede é um DoS de uma linha.
- Toda profundidade de recursão (NBT aninhado) tem limite.
- Toda descompressão tem limite de tamanho de saída (bomba de descompressão).
- Remontagem de fragmentos tem limite de memória **por sessão** e timeout.
- Fuzzing contínuo dos decoders é requisito de CI a partir do M1.

Um pacote malformado encerra a sessão de origem. Nunca o servidor.

**2. Handshake e criptografia (`bedrock-crypto`)**

- A cadeia JWT é validada até a chave raiz da Mojang. Algoritmo esperado é fixado —
  aceitar `alg` vindo do token é a vulnerabilidade clássica de JWT.
- Chaves efêmeras por sessão. Nenhum segredo é logado, nem em `debug`.
- Comparações de material criptográfico em tempo constante.
- Falha de validação é indistinguível, para o cliente, entre "assinatura inválida" e
  "identidade desconhecida".

**3. Esgotamento de recursos**

UDP não tem handshake de transporte, então qualquer um pode alegar ser qualquer origem:

- Limite de conexões por IP e limite global de conexões em abertura.
- Timeout agressivo para conexões que não completam login.
- Custo de manter uma sessão pré-autenticação mantido mínimo — nenhuma alocação grande
  antes de o cliente provar identidade.
- Rate limit de pacotes por sessão.
- **Amplificação:** nenhuma resposta a pacote não solicitado pode ser maior que o pacote
  recebido. Isso restringe o tamanho do payload de `UnconnectedPong` e é requisito de
  design, não configuração.

**4. Plugins**

Ver [ADR-005](docs/DECISIONS.md#adr-005--sandbox-de-plugins-como-requisito) e
[PLUGIN_API.md](docs/PLUGIN_API.md). Resumo: capacidades declaradas no manifesto, nada
concedido por padrão, limites de CPU e memória aplicados, plugin contido não derruba o
servidor.

Até o M3 não há sistema de plugins — e portanto não há como avaliar essa superfície além
do papel.

**5. Confiança no cliente**

Regra: **nenhuma afirmação do cliente sobre estado de jogo é aceita sem validação.**
Posição, alcance de interação, velocidade e identidade são validados pelo servidor. Isso
é postura de arquitetura desde o M0, não anti-cheat adicionado depois — retrofit de
autoridade em servidor que confiou no cliente é reescrita.

Detecção de trapaça comportamental (aimbot, padrões estatísticos) está fora de escopo.

### Fora do modelo de ameaças

- Operador malicioso. Quem controla o binário controla tudo.
- Ataques volumétricos de DDoS. Mitigação é camada de rede, não de aplicação.
- Vulnerabilidades do cliente Minecraft.
- Confidencialidade do conteúdo do mundo contra quem tem acesso ao disco.

## Prática de desenvolvimento

- `unsafe` requer justificativa por escrito no PR e revisão específica. A meta é zero.
- `cargo deny` e `cargo audit` em CI, bloqueando merge.
- Dependências novas exigem justificativa — cada uma é superfície de ataque herdada.
- Sem segredo em log, em qualquer nível.
