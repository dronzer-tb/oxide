// Transcribed verbatim from the decompiled 26.1.2 net.minecraft.util.Mth:
//   the 65536-entry SIN table and the sin/cos index arithmetic.
// Standalone so it runs on a plain JDK without building Minecraft.
public class MthOracle {
    private static final float[] SIN = new float[65536];
    static {
        for (int i = 0; i < SIN.length; i++) {
            SIN[i] = (float) Math.sin(i / 10430.378350470453);
        }
    }
    public static float sin(double i) {
        return SIN[(int) ((long) (i * 10430.378350470453) & 65535L)];
    }
    public static float cos(double i) {
        return SIN[(int) ((long) (i * 10430.378350470453 + 16384.0) & 65535L)];
    }
    public static void main(String[] args) {
        double[] angles = {
            0.0, 1.0, -1.0, 0.5, -0.5, Math.PI, Math.PI / 2, Math.PI / 4, -Math.PI,
            2 * Math.PI, 6.283185307179586, 100.0, -100.0, 1.0e6, -1.0e6,
            0.0001, -0.0001, 3.7, 12345.6789, 1.5707963267948966
        };
        for (double a : angles) {
            // Bit patterns, not decimal text: a printed float can round two distinct values
            // to the same string and hide a mismatch.
            System.out.println("mth " + Double.doubleToRawLongBits(a)
                + " " + Float.floatToRawIntBits(sin(a))
                + " " + Float.floatToRawIntBits(cos(a)));
        }
        // Every 997th table index, to cover the table itself rather than only these angles.
        for (int i = 0; i < 65536; i += 997) {
            double a = i / 10430.378350470453;
            System.out.println("mth " + Double.doubleToRawLongBits(a)
                + " " + Float.floatToRawIntBits(sin(a))
                + " " + Float.floatToRawIntBits(cos(a)));
        }
    }
}
