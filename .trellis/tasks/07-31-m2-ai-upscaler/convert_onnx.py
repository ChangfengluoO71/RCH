"""将 realesr-animevideov3.pth 转换为 ONNX 格式。"""

import torch
import torch.nn as nn
import os, collections

PTH_PATH = r'c:\Users\cfl\Desktop\RCH\app\windows\ai\models\realesr-animevideov3.pth'
ONNX_DIR = r'c:\Users\cfl\Desktop\RCH\app\windows\ai\models'

print("Loading pth...")
state = torch.load(PTH_PATH, map_location='cpu', weights_only=True)
params = state['params']

# 直接用原始 key 名构建模型: body.0..body.34 全是 sequential
sdict = collections.OrderedDict()

# body.0: Conv2d(3,64,3,1,1) + body.1: PReLU(64)
sdict['0.weight'] = params['body.0.weight']
sdict['0.bias']   = params['body.0.bias']
sdict['1.weight'] = params['body.1.weight']

# body.2-33: 16 对 Conv2d(64,64)+PReLU(64)
for i in range(16):
    ci = 2 + i * 2
    sdict[f'{ci}.weight'] = params[f'body.{ci}.weight']
    sdict[f'{ci}.bias']   = params[f'body.{ci}.bias']
    sdict[f'{ci+1}.weight'] = params[f'body.{ci+1}.weight']

# body.34: Conv2d(64,48,3,1,1) — 无 PReLU
sdict['34.weight'] = params['body.34.weight']
sdict['34.bias']   = params['body.34.bias']

print(f"Mapped {len(sdict)} params to Sequential keys")

class AnimeVideoV3(nn.Module):
    def __init__(self):
        super().__init__()
        layers = []
        # conv_first: 3→64
        layers.append(nn.Conv2d(3, 64, 3, 1, 1))
        layers.append(nn.PReLU(64))
        # mid: 16× (Conv64+PRelu)
        for _ in range(16):
            layers.append(nn.Conv2d(64, 64, 3, 1, 1))
            layers.append(nn.PReLU(64))
        # last: 64→48, NO PReLU
        layers.append(nn.Conv2d(64, 48, 3, 1, 1))
        self.body = nn.Sequential(*layers)
        self.shuffle = nn.PixelShuffle(4)

    def forward(self, x):
        feat = self.body(x)
        out = self.shuffle(feat)
        base = nn.functional.interpolate(x, scale_factor=4, mode='nearest')
        return out + base

model = AnimeVideoV3()
model.body.load_state_dict(sdict, strict=True)
model.eval()
print(f"Params: {sum(p.numel() for p in model.parameters()):,}")

# 验证
print("\nTesting inference...")
test_in = torch.randn(1, 3, 128, 128)
with torch.no_grad():
    test_out = model(test_in)
print(f"  Input:  {tuple(test_in.shape)} → Output: {tuple(test_out.shape)}")
assert test_out.shape == (1, 3, 512, 512)
print("  ✓")

# 导出
print("\nExporting ONNX (opset=17)...")
torch.onnx.export(
    model,
    torch.randn(1, 3, 480, 640),
    os.path.join(ONNX_DIR, 'realesr-animevideov3-x4.onnx'),
    input_names=['input'], output_names=['output'],
    opset_version=17,
    dynamic_axes={
        'input':  {0: 'batch', 2: 'height', 3: 'width'},
        'output': {0: 'batch', 2: 'height', 3: 'width'}
    },
)
sz = os.path.getsize(os.path.join(ONNX_DIR, 'realesr-animevideov3-x4.onnx')) / 1024 / 1024
print(f"  realesr-animevideov3-x4.onnx: {sz:.1f} MB ✓")

# ONNX Runtime 验证
import onnxruntime as ort
path = os.path.join(ONNX_DIR, 'realesr-animevideov3-x4.onnx')
sess = ort.InferenceSession(path, providers=['CPUExecutionProvider'])
d = test_in.numpy().astype('float32')
out = sess.run(None, {'input': d})[0]
print(f"    ONNX shape: {out.shape}")
diff = abs(out - test_out.numpy()).max()
print(f"    Max diff: {diff:.6f}")
assert diff < 1e-4
print("\n✅ All done!")
