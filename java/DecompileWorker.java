import com.google.gson.Gson;
import com.google.gson.JsonObject;
import jadx.api.JadxArgs;
import jadx.api.JadxDecompiler;
import jadx.api.JavaClass;
import java.io.BufferedReader;
import java.io.File;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/** JSON-lines worker (JADX failures are already inlined in the returned code, so its log is off): one request per stdin line, one response per stdout line. */
public class DecompileWorker {
    public static void main(String[] a) throws Exception {
        // JADX's logging backend writes to System.out; keep stdout reserved for the protocol.
        PrintStream protocol = new PrintStream(System.out, true, StandardCharsets.UTF_8);
        System.setOut(System.err);
        ((ch.qos.logback.classic.Logger) org.slf4j.LoggerFactory.getLogger(org.slf4j.Logger.ROOT_LOGGER_NAME))
                .setLevel(ch.qos.logback.classic.Level.OFF);

        Gson gson = new Gson();
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        String line;
        while ((line = in.readLine()) != null) {
            JsonObject resp = new JsonObject();
            try {
                JsonObject req = gson.fromJson(line, JsonObject.class);
                resp.add("id", req.get("id"));
                resp.addProperty("code", decompile(req));
            } catch (Throwable t) {
                resp.addProperty("error", t.toString());
            }
            protocol.println(gson.toJson(resp));
        }
    }

    static String decompile(JsonObject req) throws Exception {
        String target = req.get("target").getAsString();
        List<File> inputs = new ArrayList<>();
        req.getAsJsonArray("inputs").forEach(e -> inputs.add(new File(e.getAsString())));
        JadxArgs args = new JadxArgs();
        args.setInputFiles(inputs);
        args.setSkipResources(true);
        try (JadxDecompiler j = new JadxDecompiler(args)) {
            j.load();
            for (JavaClass c : j.getClasses()) {
                if (c.getRawName().equals(target)) return c.getCode();
            }
            JavaClass c = j.searchJavaClassByOrigFullName(target);
            if (c != null) return c.getCode();
        }
        throw new IllegalStateException("class not in JADX output: " + target);
    }
}
