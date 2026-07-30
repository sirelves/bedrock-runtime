# API de plugins — contrato

Este documento define **o que um plugin pode fazer**, não **como ele é executado**. O
runtime é uma decisão adiada até o M3, com benchmark
([ADR-002](DECISIONS.md#adr-002--api-de-plugins-com-runtime-adiado)).

TypeScript é usado aqui como linguagem de descrição porque tem tipos legíveis. O contrato
foi escrito para não pressupor JavaScript: nada abaixo exige que o guest seja JS, e as
assinaturas mapeiam para WASM sem mudança semântica.

Enquanto o M3 não fechar, este documento é **normativo para o design e não implementado**.
Mudanças aqui são baratas agora e caras depois — é o momento de discutir.

## Princípios

1. **Nada por padrão.** Um plugin sem permissões declaradas não lê arquivo, não abre
   socket, não vê outros plugins. Ver [ADR-005](DECISIONS.md#adr-005--sandbox-de-plugins-como-requisito).
2. **Eventos em lote, não por entidade por tick.** A API nunca oferece um callback que
   o servidor precise chamar milhares de vezes num tick. A fronteira host↔guest é cara
   em qualquer runtime; o design assume isso.
3. **Mutação é explícita e transacional.** Um plugin não recebe uma referência mutável
   ao mundo. Ele descreve mudanças; o servidor aplica no tick.
4. **Bloquear o tick é impossível por construção.** Handlers têm orçamento de tempo.
   Trabalho longo é assíncrono e não segura o tick.
5. **Erro de plugin nunca derruba o servidor.** Exceção em handler desabilita o plugin
   e registra; o servidor continua.

## Manifesto

Todo plugin declara suas capacidades antecipadamente. O servidor recusa em tempo de
carregamento o que não estiver declarado — não há escalada em runtime.

```jsonc
{
  "name": "example-plugin",
  "version": "0.1.0",
  "apiVersion": "0",           // versão do contrato desta API
  "entry": "dist/main.js",
  "permissions": {
    "world": ["read", "write"],           // ler/modificar blocos
    "players": ["read", "message", "kick"],
    "commands": ["register"],
    "storage": true,                       // KV isolado por plugin
    "network": [],                         // hosts liberados; vazio = sem rede
    "filesystem": []                       // caminhos liberados; vazio = sem FS
  },
  "limits": {
    "memoryMb": 64,
    "tickBudgetMs": 2                      // orçamento por tick; estourar suspende
  }
}
```

`apiVersion` é a versão do contrato, independente da versão do servidor. Quebra de
contrato incrementa esse número e o servidor recusa plugins de versão incompatível com
mensagem explícita.

## Ciclo de vida

```ts
export function onLoad(ctx: PluginContext): void | Promise<void>;
export function onEnable(ctx: PluginContext): void | Promise<void>;
export function onDisable(ctx: PluginContext): void | Promise<void>;
export function onUnload(ctx: PluginContext): void | Promise<void>;
```

`onLoad` e `onUnload` rodam fora do tick e podem ser assíncronos sem penalidade.
`onEnable`/`onDisable` também. Nenhum deles bloqueia o loop de jogo.

Descarregar é suportado: `onDisable` é chamado, os handlers são removidos, o estado do
plugin é descartado. Plugin que não descarrega limpo é bug do plugin, não do servidor.

## Contexto

```ts
interface PluginContext {
  readonly logger: Logger;
  readonly events: EventBus;
  readonly commands: CommandRegistry;
  readonly world: WorldView;        // presente se permissions.world != []
  readonly players: PlayerRegistry; // presente se permissions.players != []
  readonly storage: KeyValueStore;  // presente se permissions.storage
  readonly scheduler: Scheduler;
}
```

Uma capacidade não concedida **não existe** no objeto — não é uma função que lança erro.
Um plugin descobre o que pode fazer olhando o que recebeu.

## Eventos

```ts
interface EventBus {
  on<E extends keyof Events>(event: E, handler: (e: Events[E]) => void): Subscription;
}

interface Events {
  "player.join":        { player: PlayerRef };
  "player.leave":       { player: PlayerRef; reason: string };
  "player.chat":        Cancellable<{ player: PlayerRef; message: string }>;
  "block.break":        Cancellable<{ player: PlayerRef; pos: BlockPos; block: BlockRef }>;
  "block.place":        Cancellable<{ player: PlayerRef; pos: BlockPos; block: BlockRef }>;
  "player.move":        { player: PlayerRef; from: Vec3; to: Vec3 };  // amostrado, não por tick
  "server.tick":        { tick: number };                             // baixa frequência
}

type Cancellable<T> = T & { cancel(): void };
```

Notas de design:

- **`player.move` é amostrado.** Entregar cada atualização de posição de cada jogador para
  cada plugin é exatamente o padrão que a API se recusa a oferecer. A taxa de amostragem
  é configurável e documentada; quem precisa de precisão usa gatilhos de região.
- **`server.tick` não é 20 Hz.** É um gancho de baixa frequência para manutenção. Plugin
  que precisa de lógica a cada tick está resolvendo o problema errado ou deveria ser
  código do servidor.
- **Cancelamento é síncrono e imediato.** Um handler que cancela precisa fazê-lo dentro
  do orçamento; cancelar depois não é possível.

## Comandos

```ts
interface CommandRegistry {
  register(spec: CommandSpec): void;
}

interface CommandSpec {
  name: string;
  description: string;
  permission?: string;
  args: ArgSpec[];
  run(ctx: CommandContext): void | Promise<void>;
}
```

Os argumentos são declarados, não parseados pelo plugin — assim o servidor consegue
oferecer autocompletar ao cliente e validar antes de invocar o handler.

## Mundo

```ts
interface WorldView {
  getBlock(pos: BlockPos): Promise<BlockRef>;
  setBlock(pos: BlockPos, block: BlockRef): void;   // enfileira; aplica no tick
  fill(from: BlockPos, to: BlockPos, block: BlockRef): void;
}
```

`getBlock` é assíncrono porque o chunk pode não estar carregado — e carregar é I/O, que
não acontece dentro do tick ([ARCHITECTURE.md](ARCHITECTURE.md#modelo-de-concorrência)).
`setBlock` não é: descreve uma intenção que o servidor aplica, o que mantém o plugin
fora do caminho de mutação direta.

## Trabalho assíncrono

```ts
interface Scheduler {
  later(delayTicks: number, fn: () => void): TaskHandle;
  repeating(intervalTicks: number, fn: () => void): TaskHandle;
}
```

Não há criação de thread pelo plugin. Concorrência é oferecida pelo host, dentro dos
limites declarados.

## Limites e contenção

- Estourar `tickBudgetMs` repetidamente **suspende** o plugin (não mata o servidor) e
  registra o ocorrido.
- Estourar `memoryMb` desabilita o plugin.
- Laço infinito em handler é interrompido pelo host — o mecanismo depende do runtime, e
  a capacidade de fazer isso é **critério de escolha** do runtime no M3.

## Questões abertas

Deliberadamente não resolvidas até haver implementação:

- Como plugins se comunicam entre si (ou se comunicam).
- Versionamento e resolução de dependência entre plugins.
- Se existe hot reload preservando estado.
- Formato de distribuição e se haverá registry.
- Como um plugin adiciona blocos ou entidades customizadas — depende de resource packs,
  que estão fora do escopo até o M4.
