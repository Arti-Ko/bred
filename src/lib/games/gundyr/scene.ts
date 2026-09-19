// Трёхмерная сцена боя.
//
// Мир считается в двух координатах — как и прежде, в плоскости арены. Сцена
// берёт готовые позиции и раскладывает их по полу: `x` остаётся `x`, `y` мира
// становится `z` сцены. Поэтому вся механика (парирование, стойкость, фазы)
// работает ровно так же, а трёхмерным становится только взгляд.
//
// Геометрия — своя: примитивы и силуэты. Ассетов из оригинала здесь нет и быть
// не может, и дело не в лени — они чужие.

import * as THREE from 'three';

import { ARENA, type Shape } from './shapes';
import { playerRadius, type World } from './engine';

/** Мир плоский, сцена — нет: одна точка перевода на всю отрисовку. */
const toScene = (x: number, y: number): [number, number] => [x - ARENA.x, y - ARENA.y];

const GREY = {
  floor: 0x1d1d1d,
  stone: 0x262626,
  pillar: 0x303030,
  boss: 0x3c3c3c,
  bossEdge: 0x8b8b8b,
  player: 0xe9e9e9,
  steel: 0xb5b5b5,
  danger: 0xffffff,
};

export class Fight {
  readonly scene = new THREE.Scene();
  readonly camera: THREE.PerspectiveCamera;
  private readonly renderer: THREE.WebGLRenderer;

  private readonly player = new THREE.Group();
  private readonly boss = new THREE.Group();
  private readonly halberd = new THREE.Group();
  private readonly sword: THREE.Mesh;
  private readonly shield: THREE.Mesh;
  private readonly tells: THREE.Mesh[] = [];
  private readonly tellPool: THREE.Mesh[] = [];

  /** Камера догоняет цель, а не прыгает за ней: рывки тут читаются как лаги. */
  private readonly camAt = new THREE.Vector3(0, 6, 9);
  private readonly camLook = new THREE.Vector3();

  constructor(canvas: HTMLCanvasElement) {
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFSoftShadowMap;

    this.scene.background = new THREE.Color(0x050505);
    this.scene.fog = new THREE.Fog(0x050505, 90, 260);

    this.camera = new THREE.PerspectiveCamera(58, 16 / 9, 0.1, 600);

    this.buildArena();
    this.buildPlayer();
    this.buildBoss();
    this.buildLights();

    this.sword = this.player.getObjectByName('меч') as THREE.Mesh;
    this.shield = this.player.getObjectByName('щит') as THREE.Mesh;
  }

  resize(width: number, height: number): void {
    this.renderer.setSize(width, height, false);
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
  }

  dispose(): void {
    this.renderer.dispose();
    this.scene.traverse((object) => {
      if (object instanceof THREE.Mesh) {
        object.geometry.dispose();
        const material = object.material;
        if (Array.isArray(material)) material.forEach((m) => m.dispose());
        else material.dispose();
      }
    });
  }

  // ── постройка ─────────────────────────────────────────────────────────────

  private buildLights(): void {
    this.scene.add(new THREE.HemisphereLight(0x8a8a8a, 0x101010, 0.8));

    // Свет сверху и чуть сбоку: тени под ногами показывают, где ты стоишь.
    const key = new THREE.DirectionalLight(0xffffff, 1.1);
    key.position.set(40, 90, 30);
    key.castShadow = true;
    key.shadow.mapSize.set(2048, 2048);
    const box = key.shadow.camera as THREE.OrthographicCamera;
    box.left = -120;
    box.right = 120;
    box.top = 120;
    box.bottom = -120;
    box.far = 300;
    this.scene.add(key);

    // Контровой свет обводит силуэты: без него босс сливается с полом.
    const rim = new THREE.DirectionalLight(0xffffff, 0.6);
    rim.position.set(-60, 44, -70);
    this.scene.add(rim);
  }

  /** Шум на холсте вместо картинки: камню нужна фактура, а файла у нас нет. */
  private static stoneTexture(): THREE.Texture {
    const size = 512;
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext('2d')!;
    ctx.fillStyle = '#1d1d1d';
    ctx.fillRect(0, 0, size, size);
    for (let i = 0; i < 9000; i++) {
      const shade = 20 + Math.random() * 40;
      ctx.fillStyle = `rgba(${shade},${shade},${shade},0.5)`;
      ctx.fillRect(Math.random() * size, Math.random() * size, 2 + Math.random() * 3, 2);
    }
    // Плиты: швы дают масштаб, без них пол кажется бесконечной плоскостью.
    ctx.strokeStyle = 'rgba(0,0,0,0.55)';
    ctx.lineWidth = 3;
    for (let i = 0; i <= 8; i++) {
      const at = (i / 8) * size;
      ctx.beginPath();
      ctx.moveTo(at, 0);
      ctx.lineTo(at, size);
      ctx.moveTo(0, at);
      ctx.lineTo(size, at);
      ctx.stroke();
    }
    const texture = new THREE.CanvasTexture(canvas);
    texture.wrapS = THREE.RepeatWrapping;
    texture.wrapT = THREE.RepeatWrapping;
    texture.repeat.set(6, 6);
    return texture;
  }

  private buildArena(): void {
    const floor = new THREE.Mesh(
      new THREE.CircleGeometry(ARENA.r, 96),
      new THREE.MeshStandardMaterial({
        color: GREY.floor,
        map: Fight.stoneTexture(),
        roughness: 0.95,
        metalness: 0.05,
      }),
    );
    floor.rotation.x = -Math.PI / 2;
    floor.receiveShadow = true;
    this.scene.add(floor);

    // Кольца на полу: по ним видно собственное движение, как по плитам.
    for (const radius of [0.42, 0.68, 0.88]) {
      const ring = new THREE.Mesh(
        new THREE.RingGeometry(ARENA.r * radius - 0.6, ARENA.r * radius, 96),
        new THREE.MeshBasicMaterial({ color: 0xffffff, transparent: true, opacity: 0.05 }),
      );
      ring.rotation.x = -Math.PI / 2;
      ring.position.y = 0.05;
      this.scene.add(ring);
    }

    // Стена и колонны: арена должна ощущаться комнатой, а не диском в пустоте.
    const wall = new THREE.Mesh(
      new THREE.CylinderGeometry(ARENA.r + 6, ARENA.r + 6, 70, 64, 1, true),
      new THREE.MeshStandardMaterial({
        color: GREY.stone,
        roughness: 1,
        side: THREE.BackSide,
      }),
    );
    wall.position.y = 35;
    this.scene.add(wall);

    const pillar = new THREE.CylinderGeometry(6, 7, 64, 12);
    const stone = new THREE.MeshStandardMaterial({ color: GREY.pillar, roughness: 1 });
    for (let i = 0; i < 8; i++) {
      const angle = (i / 8) * Math.PI * 2 + 0.3;
      const mesh = new THREE.Mesh(pillar, stone);
      mesh.position.set(Math.cos(angle) * (ARENA.r + 2), 32, Math.sin(angle) * (ARENA.r + 2));
      mesh.castShadow = true;
      this.scene.add(mesh);
    }
  }

  private buildPlayer(): void {
    const body = new THREE.Mesh(
      new THREE.CapsuleGeometry(playerRadius() * 0.7, playerRadius() * 1.5, 6, 12),
      new THREE.MeshStandardMaterial({ color: GREY.player, roughness: 0.6 }),
    );
    body.position.y = playerRadius() * 1.5;
    body.castShadow = true;
    this.player.add(body);

    const head = new THREE.Mesh(
      new THREE.SphereGeometry(playerRadius() * 0.52, 16, 12),
      new THREE.MeshStandardMaterial({ color: 0xc9c9c9, roughness: 0.5 }),
    );
    head.position.y = playerRadius() * 2.7;
    head.castShadow = true;
    this.player.add(head);

    const sword = new THREE.Mesh(
      new THREE.BoxGeometry(1.6, 0.5, 34),
      new THREE.MeshStandardMaterial({ color: GREY.steel, roughness: 0.3, metalness: 0.7 }),
    );
    sword.name = 'меч';
    sword.position.set(9, playerRadius() * 1.6, 8);
    sword.castShadow = true;
    this.player.add(sword);

    const shield = new THREE.Mesh(
      new THREE.CylinderGeometry(9, 9, 1.4, 16),
      new THREE.MeshStandardMaterial({ color: 0x6f6f6f, roughness: 0.8 }),
    );
    shield.name = 'щит';
    shield.rotation.z = Math.PI / 2;
    shield.position.set(-10, playerRadius() * 1.7, 4);
    shield.castShadow = true;
    this.player.add(shield);

    this.scene.add(this.player);
  }

  private buildBoss(): void {
    const armour = new THREE.MeshStandardMaterial({ color: GREY.boss, roughness: 0.75 });

    const torso = new THREE.Mesh(new THREE.CapsuleGeometry(15, 26, 6, 16), armour);
    torso.position.y = 34;
    torso.castShadow = true;
    this.boss.add(torso);

    const helm = new THREE.Mesh(new THREE.SphereGeometry(10, 18, 14), armour);
    helm.position.y = 58;
    helm.castShadow = true;
    this.boss.add(helm);

    // Щель забрала — единственная светлая деталь на нём.
    const visor = new THREE.Mesh(
      new THREE.BoxGeometry(12, 1.6, 2),
      new THREE.MeshBasicMaterial({ color: GREY.bossEdge }),
    );
    visor.position.set(0, 58, 9);
    this.boss.add(visor);

    const shoulders = new THREE.Mesh(new THREE.BoxGeometry(44, 8, 18), armour);
    shoulders.position.y = 48;
    shoulders.castShadow = true;
    this.boss.add(shoulders);

    // Наплечники и гребень: по ним фигура читается как рыцарь, а не как бочка.
    for (const side of [-1, 1]) {
      const pauldron = new THREE.Mesh(new THREE.SphereGeometry(11, 14, 10), armour);
      pauldron.scale.set(1, 0.7, 1);
      pauldron.position.set(side * 20, 48, 0);
      pauldron.castShadow = true;
      this.boss.add(pauldron);
    }

    const crest = new THREE.Mesh(
      new THREE.ConeGeometry(3, 16, 4),
      new THREE.MeshStandardMaterial({ color: 0x565656, roughness: 0.9 }),
    );
    crest.position.set(0, 70, -2);
    crest.castShadow = true;
    this.boss.add(crest);

    const skirt = new THREE.Mesh(new THREE.CylinderGeometry(13, 19, 22, 14, 1, true), armour);
    skirt.position.y = 12;
    skirt.castShadow = true;
    this.boss.add(skirt);

    // Плащ: изогнутая плоскость за спиной. Он же выдаёт движение фигуры.
    const cape = new THREE.Mesh(
      new THREE.CylinderGeometry(17, 24, 46, 16, 1, true, Math.PI * 0.75, Math.PI * 0.75),
      new THREE.MeshStandardMaterial({
        color: 0x202020,
        roughness: 1,
        side: THREE.DoubleSide,
      }),
    );
    cape.position.set(0, 32, 0);
    cape.rotation.y = -Math.PI / 2;
    cape.castShadow = true;
    this.boss.add(cape);

    // Алебарда живёт в своей группе: её и вращает анимация замаха.
    const shaft = new THREE.Mesh(
      new THREE.CylinderGeometry(1.4, 1.4, 86, 8),
      new THREE.MeshStandardMaterial({ color: 0x4a4a4a, roughness: 0.9 }),
    );
    shaft.rotation.z = Math.PI / 2;
    shaft.castShadow = true;
    this.halberd.add(shaft);

    const blade = new THREE.Mesh(
      new THREE.ConeGeometry(7, 26, 4),
      new THREE.MeshStandardMaterial({ color: GREY.steel, roughness: 0.35, metalness: 0.6 }),
    );
    blade.rotation.z = -Math.PI / 2;
    blade.position.x = 54;
    blade.castShadow = true;
    this.halberd.add(blade);

    this.halberd.position.set(0, 44, 0);
    this.boss.add(this.halberd);

    this.scene.add(this.boss);
  }

  // ── кадр ──────────────────────────────────────────────────────────────────

  render(world: World, dt: number, lockOn: boolean): void {
    this.placeFigures(world);
    this.animate(world, dt);
    this.drawTells(world);
    this.moveCamera(world, dt, lockOn);
    this.renderer.render(this.scene, this.camera);
  }

  /**
   * Куда смотрит камера, в координатах мира. От этого угла отсчитывается
   * движение: «вперёд» — это вперёд по экрану, а не по оси арены.
   */
  forwardAngle(): number {
    return Math.atan2(this.camLook.z - this.camera.position.z, this.camLook.x - this.camera.position.x);
  }

  private placeFigures(world: World): void {
    const [px, pz] = toScene(world.player.at.x, world.player.at.y);
    this.player.position.set(px, 0, pz);
    this.player.rotation.y = -world.player.facing + Math.PI / 2;

    const [bx, bz] = toScene(world.boss.at.x, world.boss.at.y);
    this.boss.position.set(bx, 0, bz);
    this.boss.rotation.y = -world.boss.facing + Math.PI / 2;
  }

  /** Позы: анимации нет, но состояние должно читаться с одного взгляда. */
  private animate(world: World, dt: number): void {
    const { player, boss } = world;

    // Босс: спит на колене, замахивается — поднимает алебарду, бьёт — опускает.
    const sleeping = boss.state === 'спит' || boss.state === 'пробуждается';
    const kneeling = sleeping || boss.state === 'открыт';
    this.boss.position.y = kneeling ? -14 : 0;
    this.boss.rotation.x = kneeling ? 0.35 : 0;

    let swing = 0;
    if (boss.state === 'замах') swing = -1.1;
    else if (boss.state === 'удар') swing = 0.9;
    else if (boss.state === 'превращение') swing = -1.6;
    this.halberd.rotation.z += (swing - this.halberd.rotation.z) * Math.min(1, dt * 14);
    this.halberd.visible = !sleeping || boss.hp < 1;

    // Игрок: меч уходит вперёд на ударе, щит поднимается на блоке.
    const striking = player.state === 'удар' || player.state === 'тяжёлый' || player.state === 'риспост';
    const rolling = player.state === 'перекат';
    this.sword.rotation.x += ((striking ? -1.2 : 0) - this.sword.rotation.x) * Math.min(1, dt * 18);
    this.sword.position.z += ((striking ? 16 : 8) - this.sword.position.z) * Math.min(1, dt * 18);
    this.shield.position.z += ((player.blocking ? 12 : 4) - this.shield.position.z) * Math.min(1, dt * 14);
    this.player.position.y = rolling ? -5 : 0;
    this.player.rotation.x = rolling ? 0.7 : 0;
    // Парирование: щит выбрасывается вперёд — по этому движению его и видно.
    if (player.state === 'парирует') this.shield.position.z = 20;
  }

  /** Замахи — светлыми пятнами по полу: без них бой читается только наугад. */
  private drawTells(world: World): void {
    for (const mesh of this.tells) {
      mesh.visible = false;
      this.tellPool.push(mesh);
    }
    this.tells.length = 0;

    for (const tell of world.tells) {
      const mesh = this.tellMesh(tell.shape);
      const material = mesh.material as THREE.MeshBasicMaterial;
      material.opacity = tell.hot ? 0.5 : 0.08 + 0.22 * tell.progress;
      mesh.visible = true;
      this.tells.push(mesh);
    }
  }

  private tellMesh(shape: Shape): THREE.Mesh {
    const mesh =
      this.tellPool.pop() ??
      (() => {
        const created = new THREE.Mesh(
          new THREE.PlaneGeometry(1, 1),
          new THREE.MeshBasicMaterial({
            color: GREY.danger,
            transparent: true,
            depthWrite: false,
          }),
        );
        created.rotation.x = -Math.PI / 2;
        this.scene.add(created);
        return created;
      })();

    mesh.geometry.dispose();
    if (shape.kind === 'circle') {
      mesh.geometry = new THREE.CircleGeometry(shape.r, 40);
      const [x, z] = toScene(shape.x, shape.y);
      mesh.position.set(x, 0.12, z);
      mesh.rotation.z = 0;
    } else if (shape.kind === 'arc') {
      mesh.geometry = new THREE.CircleGeometry(shape.r, 40, -shape.spread / 2, shape.spread);
      const [x, z] = toScene(shape.x, shape.y);
      mesh.position.set(x, 0.12, z);
      mesh.rotation.z = shape.facing;
    } else {
      mesh.geometry = new THREE.PlaneGeometry(shape.len, shape.half * 2);
      const [x, z] = toScene(
        shape.x + (Math.cos(shape.facing) * shape.len) / 2,
        shape.y + (Math.sin(shape.facing) * shape.len) / 2,
      );
      mesh.position.set(x, 0.12, z);
      mesh.rotation.z = -shape.facing;
    }
    return mesh;
  }

  /**
   * Камера за плечом с захватом цели: стоит позади игрока по линии на босса и
   * смотрит между ними. Это и есть взгляд, ради которого всё затевалось.
   */
  private moveCamera(world: World, dt: number, lockOn: boolean): void {
    const [px, pz] = toScene(world.player.at.x, world.player.at.y);
    const [bx, bz] = toScene(world.boss.at.x, world.boss.at.y);

    // С захватом цели камера стоит на линии «вы — босс». Без него она просто
    // идёт за спиной: так убегают и так осматриваются.
    const dx = lockOn ? bx - px : Math.cos(world.player.facing) * 140;
    const dz = lockOn ? bz - pz : Math.sin(world.player.facing) * 140;
    const away = Math.hypot(dx, dz) || 1;
    // Камера за плечом: отходит назад по линии «босс — вы» и сдвигается вбок.
    // Без бокового смещения на ближней дистанции босс оказывается ровно за
    // вашей спиной — то есть за вашей же фигурой, и бой перестаёт читаться.
    const back = away < 140 ? 128 : 104;
    const shoulder = 30;
    const wanted = new THREE.Vector3(
      px - (dx / away) * back - (dz / away) * shoulder,
      78,
      pz - (dz / away) * back + (dx / away) * shoulder,
    );

    // Плавность выше на длинной дистанции: у самого босса камера должна быть цепкой.
    const follow = Math.min(1, dt * (away < 120 ? 7 : 4));
    this.camAt.lerp(wanted, follow);
    this.camera.position.copy(this.camAt);

    this.camLook.lerp(new THREE.Vector3(px + dx * 0.55, 32, pz + dz * 0.55), follow);
    this.camera.lookAt(this.camLook);
  }
}
